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
use tokio::io::AsyncReadExt;

use crate::agent_provider::{
    AgentProvider, ApprovalDecision, ProviderChatCapabilities, ProviderError, ProviderKind,
    ProviderRuntimeEvent, RequestId, RequestResponseFailureReason, SendTurnInput,
    SerializableProviderError, StartSessionInput, SubagentTaskKind, ThreadId, TurnId,
    UserMessageImage,
};
use crate::database::{
    AgentChatCheckpointRecord, AgentChatSessionConfig, AgentChatSessionRecord, DatabaseStore,
};
use crate::commands::usage::PlanQuotaStore;
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
    pub fn attach(&self, thread_id: &str, channel: Channel<AgentChatEventPayload>) -> u64 {
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
    // Same for a manual monitoring flag: the agent that claimed it is about to
    // be torn down, so the claim goes with it.
    state.clear_manual_monitors_for_panes(&[pane_id.clone()]);
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
        (
            "CODEMUX_WORKSPACE_ID".to_string(),
            ws.workspace_id.0.clone(),
        ),
        ("CODEMUX_PANE_ID".to_string(), pane_id.to_string()),
        (
            "CODEMUX_BROWSER_CMD".to_string(),
            "codemux browser".to_string(),
        ),
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
    // Provider history, not runtime deltas, owns accounting.
    input.recorded_usage_baseline = None;
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
            Ok(()) => eprintln!("[codemux::agent_chat] mcp prime_for_chat completed within budget"),
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
            eprintln!("[codemux::agent_chat] failed to persist session record: {error}");
        }
        // Persist a provider-returned resume cursor NOW, after the row
        // exists. The async `ResumeCursorUpdated` persist path is a plain
        // UPDATE racing this upsert across a spawned bridge task — when the
        // event wins it updates 0 rows and the cursor is silently lost,
        // leaving the FIRST dead-run rebuild with no conversation context
        // (OpenCode and Codex return their cursors at start; Claude's SDK id
        // arrives later by event). Best-effort like the neighboring persists.
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
            eprintln!("[codemux::agent_chat] failed to persist session config: {error}");
        }
        // Issue #80 — optional rollback checkpoint. Spawned AFTER the
        // provider session is up and the session row is persisted, so
        // not a single git operation (or even the settings-cache read)
        // sits on the latency-to-first-token path. The opt-in gate is
        // evaluated inside the task.
        if let Some(cwd) = cwd_for_persist.clone() {
            spawn_run_checkpoint(&app, session.thread_id.0.clone(), workspace_id.clone(), cwd);
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
    let Some(snapshot) = crate::git::git_checkpoint_create(repo_path, &ref_name, &message)? else {
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
                if let Err(error) =
                    db.delete_agent_chat_checkpoints_by_refs(&record.repo_path, &pruned)
                {
                    eprintln!(
                        "[codemux::agent_chat] failed to drop pruned checkpoint rows: {error}"
                    );
                }
            }
            Err(error) => eprintln!("[codemux::agent_chat] checkpoint prune failed: {error}"),
        }
    }
    // Re-read so the caller (and the emitted event) sees the
    // SQLite-assigned created_at.
    Ok(db.get_agent_chat_checkpoint(thread_id).or(Some(record)))
}

/// Synchronous core of the restore path. Blocking — run it on a
/// blocking thread. Public for integration tests.
pub fn restore_run_checkpoint_blocking(db: &DatabaseStore, thread_id: &str) -> Result<(), String> {
    let record = db
        .get_agent_chat_checkpoint(thread_id)
        .ok_or_else(|| "No checkpoint is recorded for this chat.".to_string())?;
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
        Some(id) if provider_kind == ProviderKind::Claude && claude_session_file_missing(id) => {
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
        recorded_usage_baseline: None,
    };

    eprintln!(
        "[codemux::agent_chat] auto-resuming dead session thread={} provider={provider_kind:?} \
         resume={} model={:?}",
        thread_id.0,
        resume_cursor.is_some(),
        record.model,
    );

    match impl_
        .start_session(build_input(resume_cursor.clone()))
        .await
    {
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
    /// User-visible, unexpanded text persisted in the transcript.
    #[serde(default)]
    pub display_text: Option<String>,
    /// Stable skill selections. Body/path data is resolved by the backend.
    #[serde(default)]
    pub skill_ids: Vec<String>,
    /// Skill-token-stripped user text before mode/effort/attachment wrappers.
    /// Native Claude commands are safe only when this equals `text`.
    #[serde(default)]
    pub skill_text: Option<String>,
    /// Discovery scope selected in Settings. Defaults on for compatibility
    /// with older clients that predate this field.
    #[serde(default = "default_include_plugins")]
    pub include_plugins: bool,
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

fn default_include_plugins() -> bool {
    true
}

fn skill_provider_for(provider: ProviderKind) -> crate::skills::SkillProvider {
    match provider {
        ProviderKind::Claude => crate::skills::SkillProvider::Claude,
        ProviderKind::Codex => crate::skills::SkillProvider::Codex,
        ProviderKind::OpenCode => crate::skills::SkillProvider::Opencode,
    }
}

fn escape_skill_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&apos;")
}

/// Materialize provider-independent selections after inventory revalidation.
fn render_skill_invocations(
    text: &str,
    skill_text: Option<&str>,
    provider: ProviderKind,
    skills: &[crate::skills::ResolvedSkillInvocation],
) -> Result<String, String> {
    if skills.is_empty() {
        return Ok(text.to_string());
    }

    if provider == ProviderKind::Claude
        && skills.len() == 1
        && skills[0].source_provider == crate::skills::SkillProvider::Claude
        && matches!(
            skills[0].invocation,
            crate::skills::SkillInvocationKind::NativeCommand
        )
    {
        if skill_text == Some(text) {
            let rest = text.trim();
            return Ok(if rest.is_empty() {
                format!("/{}", skills[0].name)
            } else {
                format!("/{} {}", skills[0].name, rest)
            });
        }
        // Mode, effort, or attachment framing must remain top-level prompt
        // context. Putting it after `/name` turns it into the command's
        // `$ARGUMENTS`, so use the readable portable envelope instead.
    }

    let mut envelopes = Vec::new();
    for skill in skills {
        if provider == ProviderKind::Codex
            && matches!(
                skill.invocation,
                crate::skills::SkillInvocationKind::CodexSkillItem
            )
            && skill.path.is_some()
        {
            continue;
        }
        let body = skill
            .body
            .as_deref()
            .filter(|body| !body.trim().is_empty())
            .ok_or_else(|| format!("skill_content_unavailable: {}", skill.name))?;
        let (base_dir, supporting_files) = match skill.base_dir.as_deref() {
            Some(base_dir) if !base_dir.is_empty() => (
                base_dir,
                "Relative scripts, references, and assets resolve from the base directory above.",
            ),
            _ => (
                "unavailable",
                "No local base directory was exposed; relative supporting files are unavailable.",
            ),
        };
        envelopes.push(format!(
            "<codemux-skill name=\"{}\" source=\"{:?}\" base-dir=\"{}\">\n{} Source-specific permissions are not grants in this session.\n\n{}\n</codemux-skill>",
            escape_skill_attribute(&skill.name),
            skill.source_provider,
            escape_skill_attribute(base_dir),
            supporting_files,
            body.trim()
        ));
    }
    if envelopes.is_empty() {
        return Ok(text.to_string());
    }
    Ok(format!("{}\n\n{}", envelopes.join("\n\n"), text)
        .trim()
        .to_string())
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
    let user_text_for_persist = input
        .display_text
        .clone()
        .unwrap_or_else(|| input.text.clone());
    let first_line = first_line_title(&user_text_for_persist);
    // Finalize the staged image refs NOW: promote each staged file into the
    // thread's dir and read its bytes back for the provider. This replaces
    // the old bytes-writing `save_chat_images` — the transcript records use
    // the FINAL paths and the provider gets real bytes. A security
    // rejection (a ref outside the permitted dirs) fails the send; a per-
    // image fs failure is logged and skipped inside the helper.
    let (saved_images, image_inputs) =
        finalize_chat_images(&thread_id_for_persist, &input.images).await?;
    let db: State<'_, DatabaseStore> = app.state();
    let skill_invocations = if input.skill_ids.is_empty() {
        Vec::new()
    } else {
        let session = db
            .get_agent_chat_session(&input.thread_id.0)
            .ok_or_else(|| "skill_resolution_session_not_found".to_string())?;
        let cwd = session
            .cwd
            .ok_or_else(|| "skill_resolution_cwd_missing".to_string())?;
        let inventory: State<'_, crate::skills::inventory::SkillInventoryService> = app.state();
        let opencode_manager: State<
            '_,
            std::sync::Arc<crate::agent_provider::opencode::OpenCodeServerManager>,
        > = app.state();
        inventory
            .resolve(
                std::path::Path::new(&cwd),
                input.include_plugins,
                skill_provider_for(provider),
                &input.skill_ids,
                Some(opencode_manager.inner()),
            )
            .await?
    };
    let rendered_text = render_skill_invocations(
        &input.text,
        input.skill_text.as_deref(),
        provider,
        &skill_invocations,
    )?;
    // Build the provider's byte-carrying input from the command DTO.
    let provider_input = SendTurnInput {
        thread_id: input.thread_id.clone(),
        text: rendered_text,
        display_text: Some(user_text_for_persist.clone()),
        images: image_inputs,
        skill_invocations,
        model_override: input.model_override.clone(),
        effort_override: input.effort_override.clone(),
        permission_mode_override: input.permission_mode_override.clone(),
        client_nonce: input.client_nonce.clone(),
    };
    // A genuine new user turn is the authoritative, provider-agnostic
    // anchor for resetting turn-scoped subagent tracking. Start a new
    // tracker turn here so a subagent left non-terminal by the previous turn
    // (e.g. Claude's background `async_launched` task that never emits a
    // terminal `task_notification`) cannot pin `review_pending`/`running`
    // and suppress `Review` for this turn and every one after (Finding 1).
    // Confirmed monitor-class tasks are deliberately carried forward: a
    // follow-up turn temporarily owns the Working dot, but the still-live
    // watch loop must reappear as Monitoring when that turn settles even if
    // the provider emits no fresh progress tick. Unlike
    // `SessionStateChanged::Running`, which Claude does not fire per user
    // turn, this path runs on every turn for all three providers.
    //
    // Follow-up queueing note: for a send that gets QUEUED behind an
    // active turn this reset runs at enqueue time (not at dispatch). Stale
    // agent entries are gone by the time the queued turn dispatches, while a
    // confirmed monitor remains truthful across the queue boundary.
    {
        let tracker: State<'_, SubagentTracker> = app.state();
        tracker.begin_turn(&thread_id_for_persist);
    }
    let result = impl_
        .send_turn(provider_input)
        .await
        .map_err(provider_err)?;

    // Best-effort: bump last_active_at so the session floats to the
    // top of the dropdown, and set an auto-title from the first user
    // turn if none exists yet. Failures here are non-fatal — a
    // missing persistence row must never block the turn.
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
            let row_id = persist_user_message(
                &db,
                &thread_id_for_persist,
                &user_text_for_persist,
                &saved_images,
                input.client_nonce.as_deref(),
            );
            // Then fan the row out to everyone watching this thread. The
            // sender already painted this bubble and drops the copy on its
            // nonce; any OTHER attached client (a second window, a paired
            // web-remote browser) needs it, both to render the turn and to
            // keep its cursor from stepping over an id it never saw. See
            // `fan_out_user_message`.
            fan_out_user_message(
                &app,
                &thread_id_for_persist,
                row_id,
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
const ACCEPTED_CHAT_IMAGE_MIME: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

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

/// The bytes + inferred media type returned by the chat image readers.
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
/// file's extension. Used by the image readers so the fallback
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

/// Identify the narrow set of raster formats the local-proof transport may
/// return. This inspects magic bytes instead of trusting a caller-controlled
/// extension, so renaming an arbitrary file to `.png` cannot turn the command
/// into a general file reader.
fn media_type_from_image_bytes(bytes: &[u8]) -> Result<String, String> {
    let media_type = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    };
    media_type
        .map(str::to_string)
        .ok_or_else(|| "validation_error: unsupported local image content".to_string())
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
fn resolve_within(root: &std::path::Path, candidate: &str) -> Result<std::path::PathBuf, String> {
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
        return Err("security_error: path escapes the permitted chat-image directory".to_string());
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
async fn move_staged_file(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
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
) -> Result<
    (
        Vec<PersistedChatImage>,
        Vec<crate::agent_provider::ImageInput>,
    ),
    String,
> {
    if refs.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let staging = chat_images_staging_dir().ok_or_else(|| "config_dir_unavailable".to_string())?;
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
                .ok_or_else(|| "validation_error: missing x-media-type header".to_string())?;
            (bytes.clone(), media_type)
        }
        InvokeBody::Json(value) => {
            let body: StageChatImageJsonBody =
                serde_json::from_value(value.clone()).map_err(|_| {
                    "validation_error: expected a raw image body or \
                     { bytes_base64, media_type }"
                        .to_string()
                })?;
            use base64::Engine as _;
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(body.bytes_base64.as_bytes())
                .map_err(|error| format!("validation_error: invalid base64 image body: {error}"))?;
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

    let staging = chat_images_staging_dir().ok_or_else(|| "config_dir_unavailable".to_string())?;
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
    let staging = chat_images_staging_dir().ok_or_else(|| "config_dir_unavailable".to_string())?;
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

/// Read a local image path referenced by an assistant's Markdown response.
///
/// Unlike [`agent_chat_read_image`], this path is not confined to Codemux's
/// persisted attachment directory: browser automation tools commonly save
/// visual proof under their own temp directory and return that absolute path
/// in the final message. Desktop WebKit loads it through the asset protocol;
/// web-remote and the browser dev mock use this bounded fallback instead.
///
/// The command accepts only absolute, existing PNG/JPEG/GIF/WebP files and
/// applies the same 25 MB ceiling as clipboard attachments. That keeps this a
/// narrow image transport rather than a general arbitrary-file read endpoint.
#[tauri::command]
pub async fn agent_chat_read_local_image<R: Runtime>(
    app: AppHandle<R>,
    path: String,
) -> Result<ChatImageBytes, String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;

    let candidate = std::path::Path::new(&path);
    if !candidate.is_absolute() {
        return Err("validation_error: local image path must be absolute".to_string());
    }

    let resolved = tokio::fs::canonicalize(candidate)
        .await
        .map_err(|error| format!("failed to resolve local image: {error}"))?;
    let metadata = tokio::fs::metadata(&resolved)
        .await
        .map_err(|error| format!("failed to inspect local image: {error}"))?;
    if !metadata.is_file() {
        return Err("validation_error: local image path is not a file".to_string());
    }
    if metadata.len() > crate::commands::files::MAX_CLIPBOARD_IMAGE_BYTES as u64 {
        return Err(format!(
            "validation_error: local image exceeds {} MB limit",
            crate::commands::files::MAX_CLIPBOARD_IMAGE_BYTES / (1024 * 1024),
        ));
    }

    let extension_media_type = media_type_from_extension(&resolved);
    if !ACCEPTED_CHAT_IMAGE_MIME.contains(&extension_media_type.as_str()) {
        return Err("validation_error: unsupported local image type".to_string());
    }

    // Limit the reader itself, not only the pre-read metadata check: the file
    // can change between stat and read, and must never make this allocation
    // grow beyond the advertised ceiling.
    let file = tokio::fs::File::open(&resolved)
        .await
        .map_err(|error| format!("failed to read local image: {error}"))?;
    let max_bytes = crate::commands::files::MAX_CLIPBOARD_IMAGE_BYTES;
    let mut bytes = Vec::with_capacity((metadata.len() as usize).min(max_bytes));
    file.take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| format!("failed to read local image: {error}"))?;
    if bytes.len() > max_bytes {
        return Err(format!(
            "validation_error: local image exceeds {} MB limit",
            max_bytes / (1024 * 1024),
        ));
    }
    let media_type = media_type_from_image_bytes(&bytes)?;
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
    static PENDING: OnceLock<Mutex<HashMap<String, PendingQueuedTurn>>> = OnceLock::new();
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
    db.append_agent_chat_message(thread_id, &payload)
        .ok()
        .flatten()
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

/// Stop the background watch loops a pane is monitoring with.
///
/// Backs the docked monitoring bar's Stop button. Up to three things happen,
/// in this order and independently of each other's success:
///
/// 1. the thread's monitor task set is cleared from [`SubagentTracker`] (which
///    also blocklists those ids for the rest of the run, so a detached task
///    that survives the interrupt cannot walk the badge back on) and the
///    resulting pane status computed;
/// 2. any manual (`codemux monitor start`) flag on the pane is cleared, so a
///    pane that was flagged both ways ends up genuinely off;
/// 3. the provider session is interrupted, best-effort, so the background tasks
///    actually die rather than being merely forgotten about.
///
/// **`thread_id` is optional, and that is the point.** The two halves of the
/// Monitoring status are independent: a pane can carry a manual flag with no
/// chat thread bound to it at all (a terminal pane, or a chat pane whose
/// session has not started). Requiring a thread would leave the Stop button on
/// such a pane doing nothing at all — pressed, spinning, never resolving.
/// Steps 1 and 3 are simply skipped when there is no thread; step 2 is the
/// part that always applies, and it is what the badge on such a pane came from.
///
/// **The interrupt is the weak link, and deliberately so.** The provider
/// interrupt path is a *turn* interrupt: with a turn in flight it cancels the
/// run and the watch loops it spawned die with it. With the turn already
/// settled — the common case, since `Monitoring` only appears after a turn
/// completes — Claude's SDK offers no "interrupt the idle session" verb, so the
/// call is a no-op and a truly detached background task can survive it. Codemux
/// still clears its own state, because the alternative (refusing to turn the
/// badge off) leaves the user with a dot they cannot dismiss. The tracker's
/// stop-blocklist is what keeps that surviving task from re-lighting the badge;
/// killing it outright needs a provider-side capability that does not exist
/// yet. See `docs/features/monitoring-status.md` § Stopping.
///
/// Errors from the interrupt are swallowed for the same reason: the user asked
/// for the monitoring to stop, and the state clear is the part Codemux can
/// always honour.
#[tauri::command]
pub async fn agent_chat_stop_monitoring<R: Runtime>(
    app: AppHandle<R>,
    provider: ProviderKind,
    thread_id: Option<ThreadId>,
    pane_id: Option<String>,
) -> Result<(), String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;

    let thread_id = thread_id.filter(|id| !id.0.is_empty());
    let state: State<'_, AppStateStore> = app.state();

    let settled = match thread_id.as_ref() {
        Some(thread_id) => {
            let tracker: State<'_, SubagentTracker> = app.state();
            tracker.stop_monitoring(&thread_id.0)
        }
        None => None,
    };

    // Prefer the caller's pane — it is the pane whose bar was pressed, and on
    // a thread-less pane it is the only thing identifying the target at all.
    let pane_id = pane_id
        .filter(|id| !id.is_empty())
        .or_else(|| thread_id.as_ref().and_then(|t| state.agent_chat_pane_id_for_thread(&t.0)));
    let manual_cleared = pane_id
        .as_deref()
        .map(|pane_id| state.stop_manual_monitor(pane_id))
        .unwrap_or(false);

    // Best-effort: a dead session (`SessionNotFound`) or a provider that
    // declines the call must not stop the state from clearing.
    if let Some(thread_id) = thread_id.as_ref() {
        let registry: State<'_, ProviderRegistry> = app.state();
        if let Ok(impl_) = lookup_provider(&registry, provider).await {
            if let Err(error) = impl_.interrupt_turn(thread_id.clone(), None).await {
                eprintln!(
                    "[codemux::agent_chat] stop_monitoring: interrupt was not accepted \
                     (state cleared anyway): {error:?}"
                );
            }
        }
    }

    // Publish whatever the pane settled to. `Review` gets the same
    // active-workspace downgrade the event path applies, so stopping a monitor
    // in the workspace you are looking at does not raise a review dot for
    // output already on screen.
    let mut status_changed = manual_cleared;
    if let (Some(thread_id), Some(mut status)) = (thread_id.as_ref(), settled) {
        if status == PaneStatus::Review && state.is_thread_pane_in_active_workspace(&thread_id.0) {
            status = PaneStatus::Idle;
        }
        status_changed |= state.set_pane_status_by_thread(&thread_id.0, status);
    }
    if status_changed {
        let snapshot = state.stamped_snapshot();
        if let Err(error) = app.emit("app-state-changed", &snapshot) {
            eprintln!("[codemux::agent_chat] failed to emit app state: {error}");
        }
    }
    Ok(())
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
            eprintln!("[codemux::agent_chat] failed to persist permission_mode config: {error}");
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
    url.starts_with("data:image/") || url.starts_with("http://") || url.starts_with("https://")
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
            eprintln!("[codemux::agent_chat] event bridge started for {kind:?}");
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

    // Plan quota is provider-scoped level state, not transcript or
    // history: intercept it, update the in-memory store, and return. Like
    // `UsageRecorded` it is never persisted and never fanned out — but
    // unlike it, nothing accumulates; each snapshot supersedes the last.
    if let ProviderRuntimeEvent::PlanUsageUpdated {
        provider,
        windows,
        plan_label,
        auth_mode,
        ..
    } = &event
    {
        let quota: State<'_, PlanQuotaStore> = app.state();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let provider_id = serde_json::to_value(*provider)
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| format!("{provider:?}").to_lowercase());
        quota.record(
            &provider_id,
            windows.clone(),
            plan_label.clone(),
            *auth_mode,
            now_ms,
        );
        return;
    }

    // Runtime usage events from older adapters are intentionally discarded.
    // Provider-owned history is the sole accounting source; writing both
    // streams would count every Codemux-launched turn twice.
    if matches!(event, ProviderRuntimeEvent::UsageRecorded { .. }) {
        return;
    }

    if let ProviderRuntimeEvent::ResumeCursorUpdated {
        thread_id,
        resume_cursor,
    } = &event
    {
        if let Some(sdk_session_id) = extract_sdk_session_id(resume_cursor) {
            let db: State<'_, DatabaseStore> = app.state();
            if let Err(error) = db.set_agent_chat_sdk_session_id(&thread_id.0, &sdk_session_id) {
                eprintln!("[codemux::agent_chat] failed to persist sdk_session_id: {error}");
            }
            // Resume creates a fresh DB row (new thread_id) carrying the
            // same sdk_session_id as the original. Collapse the
            // duplicates so the dropdown doesn't grow unboundedly.
            if let Err(error) = db.collapse_duplicate_agent_chat_sessions(&sdk_session_id) {
                eprintln!("[codemux::agent_chat] failed to collapse duplicates: {error}");
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
                eprintln!("[codemux::agent_chat] failed to clear stale sdk_session_id: {error}");
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
            // the row must still arrive as its own message.
            let row_id = persist_user_message(
                &db,
                &thread_id.0,
                text,
                &pending.images,
                pending.client_nonce.as_deref(),
            );
            // Fan the row out under its own id, so every attached client's
            // cursor moves over it instead of over it. Gated on a nonce
            // because the nonce IS the dedup key here: `TurnQueued` already
            // put a greyed bubble on every client attached at enqueue time,
            // and without a token to match it by, this copy would render a
            // second one. With it, clients holding the queued bubble skip
            // the copy, and a client that attached mid-queue inserts the
            // turn it missed (the later `QueuedTurnDispatched` then finds no
            // `queued` marker on that fresh bubble and leaves it in place).
            if pending
                .client_nonce
                .as_deref()
                .is_some_and(|n| !n.is_empty())
            {
                fan_out_user_message(
                    app,
                    &thread_id.0,
                    row_id,
                    text,
                    &pending.images,
                    pending.client_nonce.as_deref(),
                );
            }
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
                        Err(error) => {
                            eprintln!("[codemux::agent_chat] failed to persist message: {error}")
                        }
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
            eprintln!("[codemux::agent_chat] Failed to emit {AGENT_CHAT_EVENT}: {error}");
        }
        return;
    }
    fan_out_to_thread_channels(app, &thread_id, &payload);
}

/// Deliver one already-persisted payload to every channel attached to
/// `thread_id`.
///
/// Thread-scoped events (incl. the `content_delta` token stream) go over
/// the per-thread Channels only — never the global bus. An empty
/// subscriber list means no pane is currently attached to this thread;
/// transcript events are persisted before this call, so the DB hydrate
/// replays them on (re)attach. When several consumers are attached
/// (mirror mode, or a desktop window and a web-remote browser on the same
/// thread), each receives an identical copy.
///
/// Extracted so [`fan_out_user_message`] delivers on exactly the same path
/// and in the same persist-before-fan-out order as [`forward_event`],
/// rather than growing a second delivery mechanism next to it.
fn fan_out_to_thread_channels<R: Runtime>(
    app: &AppHandle<R>,
    thread_id: &ThreadId,
    payload: &AgentChatEventPayload,
) {
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

/// Fan a just-persisted user turn out to every client attached to the
/// thread, stamped with the row id it was written as.
///
/// Providers never echo user turns, so before this existed the row was
/// visible only to the client that sent it (which appended the bubble
/// optimistically) and to whoever cold-hydrated afterwards. A *second*
/// client already viewing the thread got the assistant reply — persisted
/// at a HIGHER id — as its next live event, and `applyLiveEvents` advanced
/// `lastPersistedEventId` past the user row it had never seen. Every later
/// warm tail read asks for `id > cursor`, so that user bubble was skipped
/// forever: the phone's prompt never appeared on the desktop, and no
/// amount of revisiting recovered it.
///
/// Ordering: the row is written first and its id rides on the payload, so
/// a client that also runs a tail read across the same instant dedups the
/// duplicate on `persisted_id` (`applyLiveEvents` skips anything at or
/// below its cursor) exactly as it does for assistant events. The sending
/// client drops its own copy on `client_nonce`.
///
/// Deliberately NOT routed through [`forward_event`]: that function owns
/// persistence, and this row is already on disk — re-entering it would
/// either double-write the envelope or lose the `persisted_id` stamp,
/// since `should_persist_event` returns `false` for this variant.
/// Pane-status publication and stall-watchdog activity are likewise
/// skipped: `agent_chat_send_turn` already owns the send's status
/// transition, and a user turn is an input, not provider liveness.
fn fan_out_user_message<R: Runtime>(
    app: &AppHandle<R>,
    thread_id: &str,
    persisted_id: Option<i64>,
    text: &str,
    images: &[PersistedChatImage],
    client_nonce: Option<&str>,
) {
    if thread_id.is_empty() {
        return;
    }
    let thread = ThreadId(thread_id.to_string());
    let payload = AgentChatEventPayload {
        thread_id: thread.clone(),
        event: ProviderRuntimeEvent::UserMessage {
            thread_id: thread.clone(),
            text: text.to_string(),
            images: images
                .iter()
                .map(|image| UserMessageImage {
                    path: image.path.clone(),
                    media_type: image.media_type.clone(),
                })
                .collect(),
            client_nonce: client_nonce.filter(|n| !n.is_empty()).map(str::to_string),
        },
        persisted_id,
    };
    fan_out_to_thread_channels(app, &thread, &payload);
}

// ── Per-thread running-subagent tracking ─────────────────────────────

/// How a tracked task reads to a person looking at the sidebar.
///
/// The two classifications the providers hand us are complementary rather
/// than alternative, and a single task can be both:
///
/// * `SubagentSnapshot::background_task` — derived from the launch
///   registry, so it is always available: the row is not a registered
///   top-level `Agent`/`Task` launch, which means it can outlive the turn
///   forever and must never block settlement.
/// * `SubagentSnapshot::task_kind` — precise but optional: it needs the
///   SDK to report a `task_type`, and says the row is a background watch
///   loop (a CI poll, a tailed process, a long-lived MCP monitor).
///
/// A background bash watch loop carries both, which is exactly the
/// interesting case: it settles the run *and* shows the calm badge. So the
/// tracker collapses them into one derived class per id, with `Monitor`
/// winning over the mere fact of being background.
///
/// There is deliberately no `Background` variant. A background row that is
/// not a monitor is *not tracked at all* — that is the whole point of it —
/// so a variant for it would only ever be an inert map entry that
/// [`ThreadSubagentState::is_clear`] could never drain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskClass {
    /// Real delegated agent work. Blocks settlement: the turn's `Review` is
    /// deferred until every one of these goes terminal.
    Agent,
    /// A background watch loop. Never blocks settlement, never holds the
    /// owed `Review` open, never keeps the detached browser alive — it only
    /// makes the *settled* status read `Monitoring` while it lives.
    Monitor,
}

/// What one snapshot says about its task's class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SnapshotClass {
    /// The provider was explicit.
    Known(TaskClass),
    /// The provider said nothing on *this* snapshot — the shape every
    /// `task_progress` tick has, and the only shape an SDK that predates
    /// `task_type` ever sends. Not the same as "agent": the tracker keeps
    /// whatever it already knows about the id
    /// ([`ThreadSubagentState::track`]), defaulting to agent work for an id
    /// it has never seen.
    Unreported,
    /// A background job that is not a watch loop. Never tracked at all: it
    /// can outlive the turn forever and never go terminal, so it must not
    /// be able to block the settle *or* light a badge.
    Untracked,
}

/// Fold a snapshot's two classification fields into one answer.
///
/// Order matters, and encodes the unified policy:
///
/// 1. an explicit **monitor** wins over everything — a background bash
///    watch loop is exactly that case, and it must show the badge;
/// 2. otherwise **background** wins over any agent claim, including a
///    nested subagent's, because a row that is not a registered top-level
///    launch must never be what holds the run open;
/// 3. otherwise the provider's explicit word, or silence.
fn classify_snapshot(snapshot: &crate::agent_provider::SubagentSnapshot) -> SnapshotClass {
    if snapshot.task_kind == Some(SubagentTaskKind::Monitor) {
        return SnapshotClass::Known(TaskClass::Monitor);
    }
    if snapshot.background_task {
        return SnapshotClass::Untracked;
    }
    match snapshot.task_kind {
        Some(SubagentTaskKind::Agent) => SnapshotClass::Known(TaskClass::Agent),
        Some(SubagentTaskKind::Monitor) => unreachable!("handled above"),
        None => SnapshotClass::Unreported,
    }
}

/// Per-thread bookkeeping behind the sidebar dot for a chat pane.
///
/// Two independent facts live here, and keeping them independent is the
/// whole design:
///
/// 1. **Does the run still have real work in it?** The primary chat turn
///    can finish (`TurnCompleted`) while subagents it spawned are still
///    running. The old stateless mapper published `Review` at that point,
///    dropping the spinner even though work continued in the child
///    transcripts. So `Agent`-class tasks defer the `Review` until the last
///    one goes terminal.
/// 2. **Is anything still being watched?** A `Monitor`-class task — a CI
///    watch, a tailed process, a PR poll — is the agent babysitting
///    something it has already finished with. It is calm background
///    presence, not work in flight, so it never defers anything: the run
///    settles on schedule (the `Review` publishes, `run_finished` fires,
///    the detached browser is released) and the *settled* status simply
///    reads `Monitoring` for as long as a watch loop lives. When the last
///    one ends the pane falls to the `Review`/`Idle` it would otherwise
///    have shown.
///
/// All decision logic lives in [`map_event_to_pane_status`], a pure
/// function over `(event, &mut ThreadSubagentState)` with no `AppHandle`,
/// so it is directly unit-testable.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ThreadSubagentState {
    /// Every live (`Pending`/`Running`) tracked task, with its derived
    /// class. One map rather than a set per class so an id can only ever
    /// be in one bucket — a later snapshot that reclassifies it moves it
    /// instead of double-counting it.
    tasks: std::collections::BTreeMap<String, TaskClass>,
    /// The parent turn has completed for the current run, so the dot
    /// belongs to [`Self::settled_status`] rather than to the turn.
    /// Cleared by every genuine turn boundary.
    ///
    /// This — not an owed-`Review` flag — is what makes `Monitoring`
    /// visible: a monitor whose first `Running` snapshot arrives *after*
    /// the turn settled still surfaces the badge, because the badge is
    /// derived from the live sets and this flag, never from a transition
    /// that may already have been consumed.
    turn_settled: bool,
    /// Wall clock of the last event that could still justify deferring the
    /// owed `Review`. Stamped while [`Self::review_pending`] holds and
    /// refreshed by real post-turn run activity (see
    /// [`refreshes_owed_review`]), so the watchdog can force-settle a
    /// review that is owed but has gone silent. `None` whenever no review
    /// is owed.
    review_owed_since: Option<SystemTime>,
    /// **Tombstone.** The watchdog already force-settled this thread's owed
    /// `Review` ([`SubagentTracker::take_overdue_reviews`]) while agent
    /// tasks were still tracked.
    ///
    /// The entry deliberately survives the forced settle instead of being
    /// removed. Those ids may belong to a subagent that is genuinely alive
    /// and merely quiet (one long tool call), and its real terminal
    /// snapshot can still arrive minutes later. With the entry gone that
    /// snapshot would land on a fresh `ThreadSubagentState` and leave a
    /// husk behind, tracked until session close. With the tombstone, the
    /// late terminal snapshot drains the task map normally, publishes
    /// nothing (the `Review` was already published), and the now-clear
    /// entry is dropped by [`Self::is_clear`].
    ///
    /// Cleared by every genuine turn boundary (a new
    /// `SessionStatus::Running`, session teardown, or the send-message
    /// `begin_turn`), so it never outlives the run it describes.
    forced_settled: bool,
    /// Monitor ids the user explicitly stopped this run.
    ///
    /// Stop has to be durable. The Claude demux remembers a task's kind for
    /// the whole session and re-stamps it onto every later tick, so a
    /// detached watch loop that survives the interrupt would otherwise walk
    /// straight back into the monitor set on its next `task_progress` and
    /// drift the pane back to `Monitoring`. Blocklisting the ids until the
    /// next turn boundary is what makes the button mean something.
    ///
    /// Bounded by the run, exactly like [`Self::forced_settled`]: cleared
    /// by [`Self::begin_turn`] on a new turn, by [`Self::reset`] on session
    /// close, and by the tracker-level `begin_turn` on send. It does keep the
    /// entry alive until then,
    /// which is the price of the guarantee.
    stopped_monitors: std::collections::BTreeSet<String>,
}

impl ThreadSubagentState {
    /// Whether this thread remembers nothing at all — the entry can be
    /// dropped from [`SubagentTracker`] so the map never grows unbounded.
    ///
    /// Neither the [`Self::forced_settled`] tombstone nor `turn_settled` is
    /// itself a reason to keep the entry: once the last tracked id drains
    /// out there is nothing left to disambiguate, so both are bounded by
    /// the live ids they were recorded against. A stop-blocklist *is* a
    /// reason — it has to outlive the tasks it names, or the guarantee it
    /// encodes evaporates on the very next tick.
    ///
    /// Dropping `turn_settled` with the entry does leave one edge open: a
    /// thread that settles with **nothing** tracked is collected, so a
    /// watch loop whose very first snapshot arrives after that lands on a
    /// fresh state, which assumes a turn is in flight and stays silent.
    /// That is deliberate rather than overlooked. Closing it means keeping
    /// an entry per settled thread until its next turn boundary, and
    /// re-publishing a settled status on every stray late snapshot, to buy
    /// a case the providers do not produce: tasks are started by tool calls
    /// *within* a turn, so a watch loop is always tracked before the
    /// `TurnCompleted` that settles it — which is the path
    /// [`Self::settled_status`] handles, and the one every monitor
    /// actually takes. The manual (`codemux monitor start`) half does not
    /// go through this tracker at all.
    fn is_clear(&self) -> bool {
        self.tasks.is_empty() && self.stopped_monitors.is_empty()
    }

    /// Whether any live task is real agent work, i.e. still blocking the
    /// run from settling.
    fn has_agents(&self) -> bool {
        self.tasks.values().any(|c| *c == TaskClass::Agent)
    }

    /// Whether any live task is a watch loop.
    fn has_monitors(&self) -> bool {
        self.tasks.values().any(|c| *c == TaskClass::Monitor)
    }

    /// Whether a `Review` transition is owed: the turn is over but real
    /// agent work is still holding it open.
    ///
    /// Derived rather than stored, and that derivation is the load-bearing
    /// part of the watchdog scoping: a thread whose only live entries are
    /// monitors (or background rows, which are not tracked at all) owes no
    /// `Review` by construction, so
    /// [`SubagentTracker::take_overdue_reviews`] can never force-settle it
    /// out of its `Monitoring` badge.
    fn review_pending(&self) -> bool {
        self.turn_settled && !self.forced_settled && self.has_agents()
    }

    /// Record `id` as live under `class`, resolving an unreported class
    /// against what the tracker already knows.
    ///
    /// `None` means the provider said nothing on *this* snapshot, which is
    /// not the same as "it is agent work": an already-classified id keeps
    /// its class. That is what keeps a `task_progress` tick (which never
    /// carries `task_type`) from silently demoting a monitor into the
    /// blocking set.
    ///
    /// Returns `false` when the id was refused because the user stopped it
    /// — the caller then treats the snapshot as the no-op it is.
    fn track(&mut self, id: &str, class: SnapshotClass) -> bool {
        let class = match class {
            SnapshotClass::Known(class) => class,
            SnapshotClass::Unreported => {
                self.tasks.get(id).copied().unwrap_or(TaskClass::Agent)
            }
            SnapshotClass::Untracked => {
                self.tasks.remove(id);
                return false;
            }
        };
        if class == TaskClass::Monitor && self.stopped_monitors.contains(id) {
            // The user stopped this watch loop; a surviving detached task's
            // later ticks must not resurrect the badge.
            self.tasks.remove(id);
            return false;
        }
        // Real agent work is never blocklisted — an id coming back as a
        // genuine subagent is a different claim than the one Stop refused.
        self.stopped_monitors.remove(id);
        self.tasks.insert(id.to_string(), class);
        true
    }

    /// Drop `id` from tracking, whatever class it held.
    fn untrack(&mut self, id: &str) {
        self.tasks.remove(id);
    }

    /// Start a new parent turn without forgetting confirmed live monitors.
    ///
    /// Ordinary agent entries are turn-scoped: carrying a missed terminal
    /// snapshot forward would suppress Review forever. A monitor is different:
    /// it is explicitly allowed to outlive the parent turn, and providers do
    /// not guarantee another progress snapshot merely because the user sent a
    /// follow-up. Retaining only monitor-class entries keeps the badge truthful
    /// without reviving the stale-agent bug this reset originally fixed.
    fn begin_turn(&mut self) {
        self.tasks.retain(|_, class| *class == TaskClass::Monitor);
        self.turn_settled = false;
        self.forced_settled = false;
        self.review_owed_since = None;
        self.stopped_monitors.clear();
    }

    /// Forget everything (session close / error or explicit pane teardown).
    fn reset(&mut self) {
        self.tasks.clear();
        self.turn_settled = false;
        self.forced_settled = false;
        self.review_owed_since = None;
        self.stopped_monitors.clear();
    }

    /// **The decision table**, in one place: what should the sidebar say
    /// once the parent turn is no longer the thing driving the dot?
    ///
    /// | turn settled | forced | live agents | live monitors | status       |
    /// |--------------|--------|-------------|---------------|--------------|
    /// | no           | –      | –           | –             | `None`       |
    /// | yes          | yes    | –           | –             | `None`       |
    /// | yes          | no     | ≥1          | –             | `Working`    |
    /// | yes          | no     | 0           | ≥1            | `Monitoring` |
    /// | yes          | no     | 0           | 0             | `Review`     |
    ///
    /// Read straight off the live sets — no owed-transition flag is
    /// consulted — so the badge is a fact about what is running, not a
    /// side effect of a transition that may already have been consumed.
    /// That is what lets a monitor whose first snapshot lands *after* the
    /// turn settled still light the badge, and what makes it go out the
    /// moment the last one ends.
    ///
    /// `None` mid-turn is the existing rule: the parent owns `Working`, so
    /// a metadata-only snapshot must not move the dot. `None` after a
    /// forced settle is [`Self::forced_settled`]'s rule: the watchdog
    /// already published this thread's status, and a late drain must stay
    /// silent rather than re-announcing a transition the user has seen.
    fn settled_status(&self) -> Option<PaneStatus> {
        if !self.turn_settled || self.forced_settled {
            return None;
        }
        if self.has_agents() {
            return Some(PaneStatus::Working);
        }
        if self.has_monitors() {
            return Some(PaneStatus::Monitoring);
        }
        Some(PaneStatus::Review)
    }

    /// Stop every watch loop and blocklist its id for the rest of the run.
    /// Returns whether anything was actually being monitored.
    fn stop_monitors(&mut self) -> bool {
        let stopped: Vec<String> = self
            .tasks
            .iter()
            .filter(|(_, class)| **class == TaskClass::Monitor)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &stopped {
            self.tasks.remove(id);
        }
        let had = !stopped.is_empty();
        self.stopped_monitors.extend(stopped);
        had
    }
}

/// Whether `event` is evidence that a thread's owed `Review` may still
/// legitimately be deferred — i.e. real run activity arriving after
/// `TurnCompleted`. Refreshes [`ThreadSubagentState::review_owed_since`]
/// so a genuinely busy async subagent is never force-settled by the
/// watchdog while it keeps reporting.
///
/// *Any* tick from a real subagent counts, not just a state change, which
/// is what makes the watchdog's threshold mean "every remaining blocker
/// has been silent for 10 minutes" rather than "the thread has been silent
/// since the turn ended".
///
/// A `background_task` snapshot deliberately does NOT count: those tick
/// for as long as the background job lives (forever, for a dev server) and
/// must never keep a finished run pinned to `Working`. A **monitor** tick
/// is excluded for exactly the same reason and by exactly the same logic —
/// a watch loop reports for as long as it is watching, which is the one
/// thing that must never be read as "the deliverable is still coming".
/// Both exclusions land on the same rule: only a snapshot that could still
/// be blocking the run re-arms the clock, which is precisely
/// [`TaskClass::Agent`].
///
/// Classified from the event alone rather than from the tracker, which is
/// exact in practice: the Claude demux re-stamps `task_kind` onto *every*
/// tick for a task it has classified (see `stamp_task_classification`), so
/// an unclassified tick can only come from a provider that reports no task
/// types at all — and such a provider has no monitors to mis-read.
fn refreshes_owed_review(event: &ProviderRuntimeEvent) -> bool {
    match event {
        ProviderRuntimeEvent::SubagentUpdated { subagent, .. } => matches!(
            classify_snapshot(subagent),
            SnapshotClass::Known(TaskClass::Agent) | SnapshotClass::Unreported
        ),
        ProviderRuntimeEvent::ContentDelta { .. }
        | ProviderRuntimeEvent::ItemCompleted { .. }
        | ProviderRuntimeEvent::RequestOpened { .. }
        | ProviderRuntimeEvent::RequestResolved { .. }
        | ProviderRuntimeEvent::WorkflowUpdated { .. }
        | ProviderRuntimeEvent::TasksUpdated { .. } => true,
        _ => false,
    }
}

/// Pure sweep predicate: whether `state` owes a `Review` that has gone
/// silent for at least `threshold` and should therefore be force-settled.
///
/// Three guards keep this off a healthy run:
///
/// * [`ThreadSubagentState::review_pending`] needs `turn_settled`, which
///   only `TurnCompleted` sets, so it can never fire mid-turn.
/// * It also needs a live [`TaskClass::Agent`], so a thread whose only
///   live entries are watch loops (or background rows, which are never
///   tracked) is invisible to this sweep **by construction** — the
///   watchdog can never force a `Monitoring` pane off its badge.
/// * `review_owed_since` is re-armed by *any* tick from a real agent-class
///   entry — see [`refreshes_owed_review`] — so the clock only advances
///   while **every** remaining blocker is silent.
///
/// What it therefore cannot rule out is a live-but-quiet subagent: one
/// long tool call (a `cargo test`, a big build) emits nothing for longer
/// than the threshold, and the sweep will fire on it. That is accepted
/// deliberately, and is why the forced path is non-destructive: it
/// publishes the settled status but leaves the browser session alone and
/// tombstones the entry rather than dropping it (see
/// [`SubagentTracker::take_overdue_reviews`] and [`SettleOrigin`]).
fn select_overdue_review(
    state: &ThreadSubagentState,
    now: SystemTime,
    threshold: Duration,
) -> bool {
    if !state.review_pending() {
        return false;
    }
    let Some(since) = state.review_owed_since else {
        return false;
    };
    now.duration_since(since).unwrap_or_default() >= threshold
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
    ///
    /// `now` also maintains the owed-`Review` silence clock read by
    /// [`SubagentTracker::take_overdue_reviews`].
    fn decide(
        &self,
        thread_id: &str,
        event: &ProviderRuntimeEvent,
        now: SystemTime,
    ) -> Option<PaneStatus> {
        let mut threads = self.threads.lock().expect("subagent tracker poisoned");
        let state = threads.entry(thread_id.to_string()).or_default();
        let status = map_event_to_pane_status(event, state);
        if state.review_pending() {
            if state.review_owed_since.is_none() || refreshes_owed_review(event) {
                state.review_owed_since = Some(now);
            }
        } else {
            state.review_owed_since = None;
        }
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

    /// Begin a new parent turn, dropping stale agent work while preserving
    /// confirmed live monitors. Unlike [`Self::clear_thread`], this is a turn
    /// boundary rather than a session/pane teardown.
    pub fn begin_turn(&self, thread_id: &str) {
        let mut threads = self.threads.lock().expect("subagent tracker poisoned");
        let should_remove = if let Some(state) = threads.get_mut(thread_id) {
            state.begin_turn();
            state.is_clear()
        } else {
            false
        };
        if should_remove {
            threads.remove(thread_id);
        }
    }

    /// Drop `thread_id`'s watch-loop tasks and report the status the pane
    /// should settle to, backing `agent_chat_stop_monitoring`.
    ///
    /// The user pressing Stop is a terminal signal for every monitor at once,
    /// so they are cleared wholesale instead of waiting for terminal task rows
    /// that a killed background task may never emit. Agent tasks are left
    /// alone — Stop is scoped to the monitoring bar, and silently abandoning
    /// tracking of real in-flight work would strand the spinner.
    ///
    /// The stopped ids are blocklisted for the rest of the run (see
    /// [`ThreadSubagentState::stopped_monitors`]) so a detached task that
    /// survives the provider interrupt cannot walk the badge back on with its
    /// next tick.
    ///
    /// Returns `Some(status)` to publish, or `None` when there was nothing to
    /// change — nothing monitored, or a turn still in flight that already owns
    /// the dot.
    pub fn stop_monitoring(&self, thread_id: &str) -> Option<PaneStatus> {
        let mut threads = self.threads.lock().expect("subagent tracker poisoned");
        let state = threads.entry(thread_id.to_string()).or_default();
        // Nothing was being monitored → nothing to publish. Stop is then a
        // pure no-op rather than a stray status write (and pressing it twice
        // must not repaint the dot).
        let status = if state.stop_monitors() {
            // With the monitors gone the decision table already computes the
            // right answer: `Working` if real work remains, else the settled
            // `Review` (the caller applies the usual active-workspace
            // downgrade). It abstains for a force-settled thread, which still
            // has a monitoring dot up that the user just asked to be rid of —
            // so fall back to the settled `Review` there. Mid-turn there is
            // deliberately no fallback: the turn owns the dot.
            state
                .settled_status()
                .or_else(|| state.turn_settled.then_some(PaneStatus::Review))
        } else {
            None
        };
        if state.is_clear() {
            threads.remove(thread_id);
        }
        status
    }

    /// Force-settle every thread whose owed `Review` has gone silent for
    /// at least `threshold`, returning their thread ids so the caller can
    /// publish the settled status once.
    ///
    /// The safety net behind [`map_event_to_pane_status`]: even with
    /// background tasks excluded from the blocking set, a missed terminal
    /// subagent signal (a crashed notification, a provider quirk) would
    /// otherwise pin the sidebar to "Working" forever.
    ///
    /// **Scoped to threads that are actually blocked by real agent work.**
    /// [`ThreadSubagentState::review_pending`] derives from a live
    /// [`TaskClass::Agent`], so a monitor-only thread — which settled at
    /// `TurnCompleted` and owes nothing — is never a candidate, and the
    /// sweep can never knock a pane off its `Monitoring` badge.
    ///
    /// **The entries are tombstoned, not removed.** The sweep cannot prove
    /// the tracked ids are dead — silence is not death — so it clears only
    /// what it is publishing (the silence clock) and leaves the task map
    /// intact under a [`ThreadSubagentState::forced_settled`] marker, which
    /// is also what makes `review_pending` read false from then on. That
    /// keeps the forced settle take-once (a later sweep skips it) while
    /// letting a late-but-real terminal snapshot drain the map through the
    /// ordinary path, publish nothing, and drop the entry — instead of
    /// resurrecting it as an untracked husk.
    ///
    /// Mutating under the same lock releases the mutex before the caller
    /// touches `AppStateStore`, so the two locks are never held together.
    fn take_overdue_reviews(&self, now: SystemTime, threshold: Duration) -> Vec<String> {
        let mut threads = self.threads.lock().expect("subagent tracker poisoned");
        let mut overdue = Vec::new();
        for (thread_id, state) in threads.iter_mut() {
            if !select_overdue_review(state, now, threshold) {
                continue;
            }
            state.review_owed_since = None;
            state.forced_settled = true;
            overdue.push(thread_id.clone());
        }
        // A thread force-settled with nothing left tracked has no tombstone
        // left to carry, so it is dropped like any other clear entry rather
        // than lingering in the map.
        threads.retain(|_, state| !state.is_clear());
        overdue
    }

    /// Number of threads currently tracked. Test-only: the leak the
    /// tombstone exists to prevent is only observable as map growth.
    #[cfg(test)]
    fn tracked_thread_count(&self) -> usize {
        self.threads
            .lock()
            .expect("subagent tracker poisoned")
            .len()
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
/// - the turn finished → the settled status from
///   [`ThreadSubagentState::settled_status`]: `Working` while real
///   subagents outlive the turn, `Monitoring` while only watch loops do,
///   otherwise `Review` (the caller downgrades that to `Idle` when the
///   pane's workspace is already active, matching `handle_lifecycle_event`)
/// - the session closed or errored → `Idle` (clear any stuck indicator),
///   and forget this thread's task tracking.
///
/// Task semantics (the reason this is stateful):
/// - `SubagentUpdated` with `Running`/`Pending` records the task as live
///   under its class (see [`ThreadSubagentState::track`]) and then re-reads
///   the decision table. Mid-turn that answers `None`: the parent already
///   owns `Working`, so a metadata snapshot (activity / tokens) must not
///   move the dot.
/// - `SubagentUpdated` with a terminal status (`Completed`/`Failed`/
///   `Stopped`) drops the task; the decision table then re-picks between
///   `Working`, `Monitoring` and the finally-published `Review`.
///
/// **Background tasks never block settlement.** A snapshot carrying
/// `background_task` is a provider background job (Claude emits the same
/// `system.task_*` family for `Bash { run_in_background: true }`, keyed by
/// the Bash tool_use_id). A dev server never exits, so its terminal
/// notification never arrives — treating it as agent work would defer the
/// owed `Review` forever and pin the sidebar to "Working" (and, downstream,
/// keep the background-browser `LIVE` chip up, since the release only fires
/// on a settled status). So a background row that is not also a watch loop
/// is never tracked at all, and its live snapshots return `None` — even
/// after the turn settled, so a progress tick cannot resurrect `Working`.
///
/// **Watch loops are orthogonal to all of that** (the `Monitoring` half). A
/// task the provider labelled `monitor` / `monitor_mcp` / `local_bash` /
/// `shell` is a background watch loop rather than delegated work. It does
/// **not** defer anything: the run settles exactly as it would without it,
/// so `TurnCompleted` publishes a settled status, `run_finished` fires and
/// the detached browser is released on the genuine transition. What a live
/// watch loop changes is only which settled status is shown — `Monitoring`
/// instead of `Review` — and the moment the last one ends the pane falls to
/// the `Review`/`Idle` it would otherwise have had.
///
/// A background bash watch loop is *both* things at once, which is the
/// point: it settles the run (background) and shows the calm badge
/// (monitor). There is no TTL — monitoring ends only via a terminal task
/// row, session close, or the explicit stop paths.
///
/// A genuine **new user turn** resets turn-scoped tracker state while
/// retaining confirmed monitor entries. The authoritative, provider-agnostic
/// anchor is the send-message command path ([`agent_chat_send_turn`] calls
/// [`SubagentTracker::begin_turn`]); `SessionStateChanged::Running` applies the
/// same rule here (reliably per-turn for Codex/OpenCode). The settled flag is
/// cleared at the boundary, so the parent owns `Working` during the follow-up;
/// its `TurnCompleted` reveals any retained monitor again. Session
/// `Closed`/`Error` and `agent_chat_close_pane` still perform a full clear.
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
        // Streaming output keeps the spinner alive but must NOT clear the
        // settled state. Parent-scoped output (`subagent_id == None`) can
        // still trickle in *after* a deferred `TurnCompleted` (e.g. a final
        // result item) while subagents keep running; clearing `turn_settled`
        // here would silently drop the owed `Review` and strand the pane at
        // `Working`. A genuine new turn is what resets it — via the
        // send-message command boundary (authoritative, provider-agnostic) or a
        // `SessionStateChanged::Running` snapshot — never a stray delta.
        ProviderRuntimeEvent::ContentDelta { .. }
        | ProviderRuntimeEvent::ItemCompleted { .. }
        | ProviderRuntimeEvent::RequestResolved { .. } => Some(PaneStatus::Working),
        ProviderRuntimeEvent::RequestOpened { .. } => Some(PaneStatus::Permission),
        ProviderRuntimeEvent::RequestResponseFailed { .. } => Some(PaneStatus::Review),
        ProviderRuntimeEvent::SubagentUpdated { subagent, .. } => match subagent.status {
            SubagentStatus::Pending | SubagentStatus::Running => {
                if subagent.subagent_id.is_empty() {
                    // A malformed snapshot with no id joins nothing and
                    // leaves nothing, so it must not be allowed to *conclude*
                    // anything either — above all it must never consume the
                    // owed `Review`, which is what publishing one here would
                    // do. Pre-existing behavior: hold whatever is already up.
                    return state
                        .settled_status()
                        .filter(|status| *status != PaneStatus::Review);
                }
                // A refusal — a background job that is not a watch loop, or a
                // watch loop the user already stopped — moves nothing. Not
                // even to restore `Working` after the turn settled: a
                // progress tick from something that cannot block the run must
                // not repaint the dot at all. `track` also drops any stale
                // entry for the id, covering a snapshot that arrived earlier
                // in the session before the flag was set.
                if !state.track(&subagent.subagent_id, classify_snapshot(subagent)) {
                    return None;
                }
                // Mid-turn this abstains; after the turn settled it is what
                // lights `Monitoring` for a watch loop whose first snapshot
                // only arrives now.
                state.settled_status()
            }
            SubagentStatus::Completed | SubagentStatus::Failed | SubagentStatus::Stopped => {
                state.untrack(&subagent.subagent_id);
                // A task *finishing* can only shrink the live sets, so a
                // `Working` answer here is always the spinner that is already
                // up — swallowed so an intermediate completion stays the
                // no-op it has always been. The transitions that matter
                // (Working → Monitoring, and the finally-owed Review) pass.
                //
                // The late-completion-after-a-forced-settle case is handled
                // inside the decision table, which stays silent for a
                // tombstoned thread: the watchdog already published this
                // thread's `Review`, so the real terminal snapshot must not
                // re-announce a transition the user has already seen and
                // possibly already cleared. Draining the map here is the
                // whole point — it is what lets `is_clear` drop the tombstone.
                state
                    .settled_status()
                    .filter(|status| *status != PaneStatus::Working)
            }
        },
        ProviderRuntimeEvent::TurnCompleted { .. } => {
            // The run is over as far as the parent is concerned. Whether it
            // *settles* depends only on real agent work still being tracked;
            // watch loops deliberately do not hold it open, they only change
            // which settled status is shown.
            state.turn_settled = true;
            state.settled_status()
        }
        ProviderRuntimeEvent::SessionStateChanged { status, .. } => match status {
            SessionStatus::Running { .. } => {
                // A fresh turn started: drop stale agent work, the owed-review
                // clock, forced-settle state, and the prior run's Stop
                // blocklist. A confirmed monitor is intentionally retained:
                // it can outlive the parent turn, and a provider may not emit
                // another progress snapshot just because the user followed up.
                // The parent owns Working during this turn; once it settles,
                // the retained monitor makes the pane return to Monitoring.
                // This generic event path covers Codex (`turn/started`) and
                // OpenCode (`session.status = busy`); Claude's authoritative
                // boundary is the send command above.
                state.begin_turn();
                Some(PaneStatus::Working)
            }
            SessionStatus::WaitingApproval { .. } => Some(PaneStatus::Permission),
            SessionStatus::Closed | SessionStatus::Error { .. } => {
                // Tear-down: drop all tracking so a subagent whose
                // terminal status is never observed (e.g. Claude's
                // background `async_launched` task that never emits a
                // later `task_notification`) cannot pin the spinner. Watch
                // loops go with them — the session that owned them is gone,
                // so nothing is being monitored any more.
                state.reset();
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
        // Ledger rows are the same: pure accounting, no liveness signal.
        ProviderRuntimeEvent::ContextUsageUpdated { .. }
        | ProviderRuntimeEvent::UsageRecorded { .. }
        // Plan quota is an account-level reading, not thread liveness.
        | ProviderRuntimeEvent::PlanUsageUpdated { .. } => None,
        // A fanned-out user turn is an INPUT echo, not provider liveness.
        // `agent_chat_send_turn` already owns the send's status transition
        // (and it runs on the sending client's command, before any of this),
        // so re-deriving one here would only race it.
        ProviderRuntimeEvent::UserMessage { .. } => None,
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
    let Some(status) = tracker.decide(&thread_id.0, event, SystemTime::now()) else {
        return;
    };
    apply_pane_status(app, &thread_id.0, status, SettleOrigin::ProviderEvent);
}

/// Where a pane-status write came from. The one policy that differs
/// between the two callers of [`apply_pane_status`]: whether a terminal
/// status may also tear down the workspace's detached agent browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettleOrigin {
    /// A real provider transition. The status is *evidence* the run
    /// reached that state, so the browser session it was using can go.
    ProviderEvent,
    /// The stall watchdog's backstop
    /// ([`force_settle_overdue_reviews`]). Silence is not proof of death:
    /// the run may still be alive inside one long quiet tool call, and it
    /// would then keep driving a browser session that this settle has no
    /// standing to destroy. Publishing a settled dot is recoverable — the
    /// next real event repaints it — whereas closing a live browser is
    /// not, so the forced path publishes the status and stops there.
    ForcedBackstop,
}

/// Whether a settle at `origin` reaching `status` should release the
/// workspace's detached agent browser.
///
/// Pure, so the "a forced settle never releases the browser" rule is
/// unit-testable without an `AppHandle`/`AppStateStore`.
fn should_release_browser(origin: SettleOrigin, status: PaneStatus) -> bool {
    matches!(origin, SettleOrigin::ProviderEvent) && run_finished(status)
}

/// Write `status` for `thread_id` into the shared `pane_statuses` store,
/// release the workspace's background agent browser once the run is
/// provably over, and emit a stamped `app-state-changed` snapshot when
/// either actually changed.
///
/// Extracted from [`publish_pane_status`] so the stall watchdog's forced
/// settle ([`force_settle_overdue_reviews`]) shares the Review→Idle
/// downgrade and the stamped emit instead of growing a second copy of
/// them. The browser release is the deliberate exception, gated on
/// `origin` — see [`SettleOrigin`]. Takes no tracker lock, so the caller
/// must release the tracker mutex before calling.
fn apply_pane_status<R: Runtime>(
    app: &AppHandle<R>,
    thread_id: &str,
    mut status: PaneStatus,
    origin: SettleOrigin,
) {
    let state: State<'_, AppStateStore> = app.state();
    // Mirror the terminal path: a turn that finishes in the workspace the
    // user is already looking at clears to Idle instead of nagging with a
    // review dot for output they can already see.
    //
    // Scoped to `Review` on purpose — `Monitoring` is deliberately NOT
    // downgraded. The review dot is a nag about output you can already see;
    // the monitoring dot is a live fact about a watch loop that is still
    // running, and it stays true whether or not you are looking at the pane.
    // Suppressing it in the active workspace would also hide the docked bar
    // that owns the Stop button, which is the one surface that can end it.
    if status == PaneStatus::Review && state.is_thread_pane_in_active_workspace(thread_id) {
        status = PaneStatus::Idle;
    }
    let status_changed = state.set_pane_status_by_thread(thread_id, status.clone());
    // A finished run (turn complete and settled to `Review`/`Idle`, or the
    // session closed/errored into `Idle`) releases the workspace's
    // background agent browser session, if any, so the GUI-mode chip /
    // context-bar indicator stop showing "LIVE" once the agent has nothing
    // left to do. `Working`/`Permission` must NOT release — the tracker
    // already defers `Review` while subagents are still running, so by the
    // time we see a terminal status *from a provider event* the run really
    // is over. A forced backstop settle carries no such proof and never
    // releases.
    let released = should_release_browser(origin, status)
        && state
            .agent_chat_pane_id_for_thread(thread_id)
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
/// settled to `Review`/`Monitoring`/`Idle`, or the underlying session
/// closing/erroring into `Idle`. Extracted from [`publish_pane_status`] so
/// the "should we release the background browser session" policy is
/// unit-testable without an `AppHandle`/`AppStateStore`. `Working` and
/// `Permission` are mid-run and must return `false`.
///
/// `Monitoring` counts as finished, and that is the deliberate call. A watch
/// loop is not the run: the deliverable is done and the turn has settled, so
/// the browser session the run was using has no owner left. Letting a monitor
/// hold it would put the "is the run over?" question back at the mercy of a
/// task that may never terminate — precisely the failure the background-task
/// classification exists to prevent, and it would leave the GUI-mode `LIVE`
/// chip up indefinitely. A monitor that genuinely needs a browser opens one;
/// it does not get to squat on the finished run's.
fn run_finished(status: PaneStatus) -> bool {
    matches!(
        status,
        PaneStatus::Review | PaneStatus::Monitoring | PaneStatus::Idle
    )
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
            status: SessionStatus::Ready | SessionStatus::Closed | SessionStatus::Error { .. },
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

/// Force-settle every thread whose owed `Review` has gone silent past
/// [`STALL_THRESHOLD`], publishing the settled pane status through
/// [`apply_pane_status`] (Review, downgraded to Idle in the active
/// workspace, plus the stamped app-state emit).
///
/// This is the backstop for a missed terminal subagent signal: without it a
/// thread that completed its turn while something was still tracked as
/// running stays "Working" indefinitely, because the only exit is a
/// terminal snapshot that never arrives.
///
/// **Deliberately non-destructive.** The trigger is silence, and silence
/// cannot distinguish a dead subagent from a live one sitting inside one
/// long tool call. So the forced path only ever writes a pane status:
///
/// * it passes [`SettleOrigin::ForcedBackstop`], which withholds the
///   detached-browser release — tearing down the browser under a live
///   subagent would be unrecoverable, while a premature dot is repainted
///   by the next real event;
/// * `take_overdue_reviews` tombstones rather than removes the tracker
///   entry, so a late real completion drains normally, publishes nothing,
///   and leaves no orphaned entry behind.
///
/// `review_pending` is set exclusively by `TurnCompleted`, so a mid-flight
/// turn can never be cut short here, and every remaining blocker must have
/// been silent for the full threshold (see [`refreshes_owed_review`]).
///
/// The tracker mutex is released by `take_overdue_reviews` before any
/// `AppStateStore` call, so the two locks are never held together.
fn force_settle_overdue_reviews<R: Runtime>(app: &AppHandle<R>, now: SystemTime) {
    let overdue = {
        let tracker: State<'_, SubagentTracker> = app.state();
        tracker.take_overdue_reviews(now, STALL_THRESHOLD)
    };
    for thread_id in overdue {
        eprintln!(
            "[codemux::agent_chat] force-settling an overdue Review for thread {thread_id} (turn completed, no subagent activity for {}s)",
            STALL_THRESHOLD.as_secs()
        );
        apply_pane_status(app, &thread_id, PaneStatus::Review, SettleOrigin::ForcedBackstop);
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
/// The same sweep also force-settles an **owed `Review`** that has gone
/// silent for the same threshold (see [`force_settle_overdue_reviews`]).
/// That case is not a stall — the turn already completed — so it does more
/// than advise: it publishes the settled status the run never got. It stops
/// there, though. It never releases the detached browser and never drops
/// tracker state, because silence alone cannot prove the run is dead.
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
                let wall_delta = now_wall.duration_since(prev_wall).unwrap_or_default();
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
            // Second pass: a turn that already completed but never got its
            // owed `Review` published. Runs after the stall pass so a
            // forced settle is never masked by the advisory notice.
            force_settle_overdue_reviews(&app, now_wall);
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
    // NOTE: `UserMessage` is deliberately NOT persisted here either — it is
    // MINTED from a row `persist_user_message` has already written, and
    // re-persisting it in `forward_event` would double the bubble on every
    // hydrate. Its `persisted_id` is stamped by `fan_out_user_message`
    // instead, which is why that path bypasses `forward_event`.
    //
    // NOTE: `RunStalled` is deliberately NOT persisted. It is a transient
    // advisory recomputed live by the stall watchdog; the durable record of
    // a dead run is the settled/`child_exited` `TurnCompleted`, which drives
    // the "Run interrupted" divider on hydrate.
}

/// Pull the SDK session UUID out of the opaque `resume_cursor` JSON.
///
/// Providers wrap the id under a provider-native key (`threadId` for Codex,
/// `sessionId` for Claude) or the generic `resume` key accepted when a
/// persisted scalar is reconstructed. Extracted so the logic is unit-testable
/// without spinning up an app handle.
pub fn extract_sdk_session_id(cursor: &serde_json::Value) -> Option<String> {
    cursor
        .get("resume")
        .or_else(|| cursor.get("sessionId"))
        .or_else(|| cursor.get("session_id"))
        .or_else(|| cursor.get("threadId"))
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
        | ProviderRuntimeEvent::UserMessage { thread_id, .. }
        | ProviderRuntimeEvent::UsageRecorded { thread_id, .. }
        | ProviderRuntimeEvent::PlanUsageUpdated { thread_id, .. }
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
            value["item"]["tool_name"] = json!("N".repeat(LAZY_TOOL_RESULT_THRESHOLD_BYTES + 100));
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
        assert_eq!(validate_chat_image_mime("image/png").unwrap(), "image/png");
        assert_eq!(
            validate_chat_image_mime("image/jpeg").unwrap(),
            "image/jpeg"
        );
        assert_eq!(validate_chat_image_mime("image/gif").unwrap(), "image/gif");
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
        assert_eq!(
            validate_chat_image_mime("Image/WebP").unwrap(),
            "image/webp"
        );
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

    #[test]
    fn media_type_from_image_bytes_uses_magic_not_the_filename() {
        assert_eq!(
            media_type_from_image_bytes(b"\x89PNG\r\n\x1a\nrest").unwrap(),
            "image/png"
        );
        assert_eq!(
            media_type_from_image_bytes(b"GIF89a-rest").unwrap(),
            "image/gif"
        );
        assert!(media_type_from_image_bytes(b"private text wearing a .png suffix").is_err());
    }

    // ── chat image staging: path containment (security boundary) ──

    #[test]
    fn resolve_within_accepts_a_path_inside_root() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("img.png");
        std::fs::write(&file, b"x").unwrap();
        let resolved = resolve_within(root.path(), file.to_str().unwrap()).unwrap();
        assert_eq!(resolved, file.canonicalize().unwrap());
    }

    #[test]
    fn resolve_within_accepts_a_path_in_a_subdir_of_root() {
        let root = tempfile::tempdir().unwrap();
        let sub = root.path().join("thread-1");
        std::fs::create_dir_all(&sub).unwrap();
        let file = sub.join("img.png");
        std::fs::write(&file, b"x").unwrap();
        let resolved = resolve_within(root.path(), file.to_str().unwrap()).unwrap();
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

        match classify_chat_image_ref(&staging, &thread_dir, staged.to_str().unwrap()).unwrap() {
            ChatImageRefLocation::Staged(p) => {
                assert_eq!(p, staged.canonicalize().unwrap())
            }
            ChatImageRefLocation::ThreadLocal(_) => panic!("expected Staged"),
        }
        match classify_chat_image_ref(&staging, &thread_dir, local.to_str().unwrap()).unwrap() {
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
        assert!(classify_chat_image_ref(&staging, &thread_dir, foreign.to_str().unwrap()).is_err());

        // A file entirely outside the tree is rejected.
        let outside_dir = tempfile::tempdir().unwrap();
        let outside = outside_dir.path().join("o.png");
        std::fs::write(&outside, b"x").unwrap();
        assert!(classify_chat_image_ref(&staging, &thread_dir, outside.to_str().unwrap()).is_err());
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

    // ── user-turn fan-out (cross-client cursor safety) ──
    //
    // Providers never echo user turns, so the persisted row is the only
    // record of one. Before the fan-out, a second client attached to the
    // thread received the ASSISTANT reply (persisted at a higher id) as its
    // next live event and advanced `lastPersistedEventId` past a user row it
    // had never seen — after which every `id > cursor` tail read skipped that
    // row forever. These tests pin the two properties that make the fix work:
    // the live payload is byte-identical to the stored envelope, and it
    // carries the row's own id.

    /// The fanned-out event and the persisted envelope MUST serialize
    /// identically. The frontend folds both through one reducer case, so any
    /// drift here silently becomes "the live bubble and the replayed bubble
    /// disagree" — the exact class of bug the shared shape exists to prevent.
    #[test]
    fn user_message_event_serializes_like_the_persisted_envelope() {
        let images = vec![PersistedChatImage {
            path: "/tmp/a.png".into(),
            media_type: "image/png".into(),
        }];
        let event = ProviderRuntimeEvent::UserMessage {
            thread_id: ThreadId("t1".into()),
            text: "look".into(),
            images: images
                .iter()
                .map(|image| UserMessageImage {
                    path: image.path.clone(),
                    media_type: image.media_type.clone(),
                })
                .collect(),
            client_nonce: None,
        };
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            user_message_envelope("t1", "look", &images)
        );
    }

    /// No images and no nonce → the fields are ABSENT, not `null`, matching
    /// the pre-images transcript shape byte for byte.
    #[test]
    fn user_message_event_omits_empty_images_and_nonce() {
        let event = ProviderRuntimeEvent::UserMessage {
            thread_id: ThreadId("t1".into()),
            text: "hello".into(),
            images: Vec::new(),
            client_nonce: None,
        };
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value, user_message_envelope("t1", "hello", &[]));
        assert!(value.get("images").is_none());
        assert!(value.get("client_nonce").is_none());
    }

    /// The whole point: every client attached to the thread gets the turn,
    /// stamped with the row id it was persisted as (so cursors advance over
    /// it rather than past it) and with the sender's nonce (so the sender
    /// drops its own duplicate).
    #[test]
    fn fan_out_user_message_reaches_every_attached_client_with_row_id() {
        let app = tauri::test::mock_app();
        app.manage(AgentChatChannelRegistry::default());
        let handle = app.handle().clone();
        let registry: State<'_, AgentChatChannelRegistry> = handle.state();

        // Two clients on one thread — e.g. the desktop window and a paired
        // web-remote browser — plus a third on an unrelated thread.
        let (desktop, desktop_rx) = capture_channel();
        let (web, web_rx) = capture_channel();
        let (other, other_rx) = capture_channel();
        registry.attach("t1", desktop);
        registry.attach("t1", web);
        registry.attach("t2", other);

        let images = vec![PersistedChatImage {
            path: "/tmp/a.png".into(),
            media_type: "image/png".into(),
        }];
        fan_out_user_message(
            &handle,
            "t1",
            Some(42),
            "hi there",
            &images,
            Some("nonce-1"),
        );

        for captured in [&desktop_rx, &web_rx] {
            let received = captured.lock().unwrap();
            assert_eq!(received.len(), 1, "one copy per attached client");
            assert_eq!(received[0].thread_id.0, "t1");
            assert_eq!(
                received[0].persisted_id,
                Some(42),
                "the row id must ride along or the cursor cannot advance over it"
            );
            match &received[0].event {
                ProviderRuntimeEvent::UserMessage {
                    text,
                    client_nonce,
                    images,
                    ..
                } => {
                    assert_eq!(text, "hi there");
                    assert_eq!(client_nonce.as_deref(), Some("nonce-1"));
                    assert_eq!(images.len(), 1);
                    assert_eq!(images[0].path, "/tmp/a.png");
                }
                other => panic!("unexpected event variant: {other:?}"),
            }
        }
        assert!(
            other_rx.lock().unwrap().is_empty(),
            "another thread's clients must not see this turn"
        );
    }

    /// An empty thread id is a no-op rather than a broadcast — the same
    /// guard `forward_event` applies before touching the channel registry.
    #[test]
    fn fan_out_user_message_ignores_an_empty_thread_id() {
        let app = tauri::test::mock_app();
        app.manage(AgentChatChannelRegistry::default());
        let handle = app.handle().clone();
        let registry: State<'_, AgentChatChannelRegistry> = handle.state();
        let (channel, captured) = capture_channel();
        registry.attach("", channel);

        fan_out_user_message(&handle, "", Some(1), "hi", &[], None);

        assert!(captured.lock().unwrap().is_empty());
    }

    /// `forward_event` must never re-persist a fanned-out user turn: the row
    /// already exists (that is where the event came from), so persisting it
    /// again would double the bubble on the next hydrate.
    #[test]
    fn user_message_is_not_persisted_by_forward_event() {
        assert!(!should_persist_event(&ProviderRuntimeEvent::UserMessage {
            thread_id: ThreadId("t1".into()),
            text: "hi".into(),
            images: Vec::new(),
            client_nonce: None,
        }));
    }

    #[test]
    fn extract_sdk_session_id_handles_all_provider_keys() {
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
        assert_eq!(
            extract_sdk_session_id(&json!({"threadId": "codex-thread"})),
            Some("codex-thread".into())
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
            pr_head_branch: None,
            provider_kind: None,
            linked_issue: None,
            notifications_muted: false,
            pinned_at: None,
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
            item: CompletedItem::AssistantText { text: "hi".into() },
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
            status: SessionStatus::Running {
                active_turn: turn(),
            },
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
            ActivityUpdate::Set(ThreadActivity {
                mid_turn: true,
                waiting_approval: false,
                ..
            })
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
            ActivityUpdate::Set(ThreadActivity {
                waiting_approval: true,
                ..
            })
        ));
        let waiting = ProviderRuntimeEvent::SessionStateChanged {
            thread_id: tid(),
            status: SessionStatus::WaitingApproval { request_id: req() },
        };
        assert!(matches!(
            activity_update(None, &waiting, now),
            ActivityUpdate::Set(ThreadActivity {
                waiting_approval: true,
                ..
            })
        ));
        // Resolving the approval un-pauses the clock.
        let resolved = ProviderRuntimeEvent::RequestResolved {
            thread_id: tid(),
            request_id: req(),
            decision: ApprovalDecision::AllowForSession,
        };
        assert!(matches!(
            activity_update(None, &resolved, now),
            ActivityUpdate::Set(ThreadActivity {
                waiting_approval: false,
                mid_turn: true,
                ..
            })
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
        assert_eq!(
            activity_update(None, &completed, now),
            ActivityUpdate::Remove
        );
        for status in [
            SessionStatus::Ready,
            SessionStatus::Closed,
            SessionStatus::Error {
                message: "x".into(),
            },
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
            select_stalled(
                &a,
                base + STALL_THRESHOLD - Duration::from_secs(1),
                STALL_THRESHOLD
            ),
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
        assert_eq!(
            stalled,
            vec![("thread-a".to_string(), STALL_THRESHOLD.as_secs())]
        );
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
        let captured: Arc<Mutex<Vec<AgentChatEventPayload>>> = Arc::new(Mutex::new(Vec::new()));
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
    fn fan_out(registry: &AgentChatChannelRegistry, thread: &str, payload: AgentChatEventPayload) {
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

        assert!(
            registry.detach("t1", g1),
            "matching detach removes the entry"
        );

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
                    status: SessionStatus::Running {
                        active_turn: turn()
                    },
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
                    status: SessionStatus::Error {
                        message: "boom".into()
                    },
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

    // ── Monitoring: watch-loop tasks vs agent tasks ──────────────────
    //
    // These mirror the deferred-review tests above, but for the second
    // class: a task the provider labelled as a background watch loop makes
    // the *settled* status read `Monitoring` without ever deferring the
    // settle itself.

    fn monitor_event(id: &str, status: SubagentStatus) -> ProviderRuntimeEvent {
        ProviderRuntimeEvent::SubagentUpdated {
            thread_id: tid(),
            subagent: SubagentSnapshot {
                subagent_id: id.into(),
                status,
                task_kind: Some(SubagentTaskKind::Monitor),
                ..Default::default()
            },
        }
    }

    /// The interesting intersection: a `Bash { run_in_background: true }`
    /// watch loop. Both classifications are true of it at once.
    fn background_monitor_event(id: &str, status: SubagentStatus) -> ProviderRuntimeEvent {
        ProviderRuntimeEvent::SubagentUpdated {
            thread_id: tid(),
            subagent: SubagentSnapshot {
                subagent_id: id.into(),
                status,
                task_kind: Some(SubagentTaskKind::Monitor),
                background_task: true,
                ..Default::default()
            },
        }
    }

    /// A snapshot with NO `task_kind` — the shape every `task_progress`
    /// tick has, and the shape an SDK that predates the field always has.
    fn unclassified_event(id: &str, status: SubagentStatus) -> ProviderRuntimeEvent {
        subagent_event(id, status)
    }

    #[test]
    fn monitor_only_tasks_settle_the_run_and_read_as_monitoring() {
        let mut st = ThreadSubagentState::default();
        assert_eq!(
            map_event_to_pane_status(&monitor_event("m1", SubagentStatus::Running), &mut st),
            None,
            "mid-turn the parent still owns Working"
        );
        assert_eq!(
            map_event_to_pane_status(&turn_completed(), &mut st),
            Some(PaneStatus::Monitoring),
            "deliverable done, watch loop still live"
        );
        // The run genuinely settled: no `Review` is owed, so nothing can
        // hold the spinner and the watchdog has nothing to sweep.
        assert!(
            !st.review_pending(),
            "a watch loop must never hold a Review open"
        );
        assert_eq!(st.tasks.get("m1"), Some(&TaskClass::Monitor));
        assert!(!st.has_agents());
    }

    // The intersection case #254 and #251 disagreed about: a background
    // bash watch loop. It must settle the run (so `run_finished` fires and
    // the detached browser is released) *and* show the calm badge.
    #[test]
    fn a_background_watch_loop_settles_the_run_and_still_shows_the_badge() {
        let mut st = ThreadSubagentState::default();
        map_event_to_pane_status(
            &background_monitor_event("bg", SubagentStatus::Running),
            &mut st,
        );
        assert_eq!(
            map_event_to_pane_status(&turn_completed(), &mut st),
            Some(PaneStatus::Monitoring)
        );
        assert!(!st.review_pending());
        assert!(
            run_finished(PaneStatus::Monitoring),
            "the settle must release the browser like any other finished run"
        );
        // And it goes out when the watch loop does.
        assert_eq!(
            map_event_to_pane_status(
                &background_monitor_event("bg", SubagentStatus::Completed),
                &mut st
            ),
            Some(PaneStatus::Review),
            "the pane falls to the settled status it would otherwise show"
        );
        assert!(st.is_clear());
    }

    #[test]
    fn agent_task_alongside_a_monitor_still_reports_working() {
        let mut st = ThreadSubagentState::default();
        map_event_to_pane_status(&monitor_event("m1", SubagentStatus::Running), &mut st);
        map_event_to_pane_status(&subagent_event("s1", SubagentStatus::Running), &mut st);
        assert_eq!(
            map_event_to_pane_status(&turn_completed(), &mut st),
            Some(PaneStatus::Working),
            "real work outranks a watch loop"
        );
        assert!(st.review_pending(), "the agent task owes the Review");
        // The agent task finishes; only the monitor is left.
        assert_eq!(
            map_event_to_pane_status(&subagent_event("s1", SubagentStatus::Completed), &mut st),
            Some(PaneStatus::Monitoring),
            "Working hands over to Monitoring, not to Review"
        );
        assert!(!st.review_pending());
    }

    #[test]
    fn the_last_monitor_ending_falls_back_to_the_settled_review() {
        let mut st = ThreadSubagentState::default();
        map_event_to_pane_status(&monitor_event("m1", SubagentStatus::Running), &mut st);
        map_event_to_pane_status(&monitor_event("m2", SubagentStatus::Running), &mut st);
        map_event_to_pane_status(&turn_completed(), &mut st);

        assert_eq!(
            map_event_to_pane_status(&monitor_event("m1", SubagentStatus::Completed), &mut st),
            Some(PaneStatus::Monitoring),
            "one watch loop left — still monitoring"
        );
        assert_eq!(
            map_event_to_pane_status(&monitor_event("m2", SubagentStatus::Stopped), &mut st),
            Some(PaneStatus::Review),
            "the last watch loop stops — the pane falls to Review"
        );
        assert!(st.is_clear());
    }

    // Finding 4: the badge is derived from the live sets, never from an
    // owed transition — so a monitor whose first snapshot lands *after*
    // the turn already settled still lights it, and clears it again.
    #[test]
    fn a_monitor_starting_after_the_turn_settled_still_surfaces_monitoring() {
        let mut st = ThreadSubagentState::default();
        // A clean turn with nothing outstanding: Review published, done.
        assert_eq!(
            map_event_to_pane_status(&turn_completed(), &mut st),
            Some(PaneStatus::Review)
        );
        assert!(!st.review_pending(), "nothing was ever owed");
        // Only now does the watch loop report itself.
        assert_eq!(
            map_event_to_pane_status(&monitor_event("late", SubagentStatus::Running), &mut st),
            Some(PaneStatus::Monitoring),
            "the badge must not depend on a pending Review"
        );
        assert_eq!(
            map_event_to_pane_status(&monitor_event("late", SubagentStatus::Completed), &mut st),
            Some(PaneStatus::Review),
            "and it clears when the monitor ends"
        );
    }

    #[test]
    fn an_unclassified_tick_does_not_demote_a_known_monitor() {
        // `task_progress` carries no `task_type`. If `None` were read as
        // "agent", the very first progress tick would move the task back
        // into the blocking set and pin the pane at Working forever.
        let mut st = ThreadSubagentState::default();
        map_event_to_pane_status(&monitor_event("m1", SubagentStatus::Running), &mut st);
        map_event_to_pane_status(&turn_completed(), &mut st);
        assert_eq!(
            map_event_to_pane_status(&unclassified_event("m1", SubagentStatus::Running), &mut st),
            Some(PaneStatus::Monitoring)
        );
        assert_eq!(st.tasks.get("m1"), Some(&TaskClass::Monitor));
        assert!(!st.has_agents());
    }

    #[test]
    fn an_explicit_reclassification_moves_the_id_between_classes() {
        let mut st = ThreadSubagentState::default();
        map_event_to_pane_status(&subagent_event("x", SubagentStatus::Running), &mut st);
        map_event_to_pane_status(&turn_completed(), &mut st);
        assert_eq!(st.tasks.get("x"), Some(&TaskClass::Agent));
        // A later snapshot says it is really a watch loop.
        assert_eq!(
            map_event_to_pane_status(&monitor_event("x", SubagentStatus::Running), &mut st),
            Some(PaneStatus::Monitoring)
        );
        assert_eq!(st.tasks.get("x"), Some(&TaskClass::Monitor));
        assert_eq!(st.tasks.len(), 1, "no double-counting across classes");
    }

    #[test]
    fn a_new_turn_preserves_monitors_but_session_close_clears_them() {
        let mut st = ThreadSubagentState::default();
        map_event_to_pane_status(&monitor_event("m1", SubagentStatus::Running), &mut st);
        map_event_to_pane_status(&turn_completed(), &mut st);

        assert_eq!(
            map_event_to_pane_status(&session_running(), &mut st),
            Some(PaneStatus::Working),
            "the follow-up turn temporarily owns the dot"
        );
        assert_eq!(st.tasks.get("m1"), Some(&TaskClass::Monitor));
        assert!(!st.turn_settled);
        assert_eq!(
            map_event_to_pane_status(&turn_completed(), &mut st),
            Some(PaneStatus::Monitoring),
            "the still-live monitor must reappear without a fresh provider tick"
        );

        assert_eq!(
            map_event_to_pane_status(
                &ProviderRuntimeEvent::SessionStateChanged {
                    thread_id: tid(),
                    status: SessionStatus::Closed,
                },
                &mut st,
            ),
            Some(PaneStatus::Idle)
        );
        assert!(st.tasks.is_empty(), "session close must drop watch loops");
        assert!(st.is_clear());
    }

    // Finding 7: a `Running` snapshot with an empty subagent_id is
    // malformed — it joins nothing and leaves nothing, so it must never be
    // the thing that publishes (and consumes) the owed Review.
    #[test]
    fn an_empty_subagent_id_never_publishes_or_consumes_a_review() {
        let empty = |status| {
            ProviderRuntimeEvent::SubagentUpdated {
                thread_id: tid(),
                subagent: SubagentSnapshot {
                    subagent_id: String::new(),
                    status,
                    ..Default::default()
                },
            }
        };

        // Mid-turn: no-op, exactly as before.
        let mut st = ThreadSubagentState::default();
        assert_eq!(map_event_to_pane_status(&empty(SubagentStatus::Running), &mut st), None);
        assert!(st.tasks.is_empty(), "nothing may be tracked under an empty id");

        // After a settle with a real subagent still live: hold the spinner.
        let mut st = ThreadSubagentState::default();
        map_event_to_pane_status(&subagent_event("s1", SubagentStatus::Running), &mut st);
        map_event_to_pane_status(&turn_completed(), &mut st);
        assert_eq!(
            map_event_to_pane_status(&empty(SubagentStatus::Running), &mut st),
            Some(PaneStatus::Working)
        );
        assert!(st.review_pending(), "the owed Review survives untouched");

        // After a settle with nothing live: silence, NOT a Review.
        let mut st = ThreadSubagentState::default();
        map_event_to_pane_status(&monitor_event("m1", SubagentStatus::Running), &mut st);
        map_event_to_pane_status(&turn_completed(), &mut st);
        assert_eq!(
            map_event_to_pane_status(&empty(SubagentStatus::Running), &mut st),
            Some(PaneStatus::Monitoring),
            "a live watch loop is still the truth"
        );
        map_event_to_pane_status(&monitor_event("m1", SubagentStatus::Completed), &mut st);
        assert_eq!(
            map_event_to_pane_status(&empty(SubagentStatus::Running), &mut st),
            None,
            "a malformed snapshot must not announce a transition of its own"
        );
    }

    // The same race, driven through the real tracker rather than a bare
    // state — which is the path that also has to survive the entry
    // bookkeeping. A watch loop is always tracked before the
    // `TurnCompleted` that settles it (tasks are started by tool calls
    // *within* a turn), so the entry stays alive across the settle and the
    // badge holds until the monitor actually ends.
    #[test]
    fn the_tracker_holds_the_badge_across_a_settle_and_drops_it_after() {
        let tracker = SubagentTracker::default();
        let now = SystemTime::now();
        assert_eq!(
            tracker.decide("t", &monitor_event("m1", SubagentStatus::Running), now),
            None,
            "mid-turn the parent owns the dot"
        );
        assert_eq!(
            tracker.decide("t", &turn_completed(), now),
            Some(PaneStatus::Monitoring)
        );
        // Ticks keep answering Monitoring rather than falling back.
        assert_eq!(
            tracker.decide("t", &monitor_event("m1", SubagentStatus::Running), now),
            Some(PaneStatus::Monitoring)
        );
        assert_eq!(
            tracker.tracked_thread_count(),
            1,
            "the entry survives the settle because the watch loop does"
        );
        assert_eq!(
            tracker.decide("t", &monitor_event("m1", SubagentStatus::Completed), now),
            Some(PaneStatus::Review),
            "and the pane falls to the settled status when it ends"
        );
        assert_eq!(tracker.tracked_thread_count(), 0);
    }

    #[test]
    fn stop_monitoring_clears_watch_loops_and_settles_the_pane() {
        let tracker = SubagentTracker::default();
        let now = SystemTime::now();
        tracker.decide("t1", &monitor_event("m1", SubagentStatus::Running), now);
        tracker.decide("t1", &turn_completed(), now);
        assert_eq!(tracker.stop_monitoring("t1"), Some(PaneStatus::Review));
        // A second Stop is a no-op rather than a stray status write.
        assert_eq!(tracker.stop_monitoring("t1"), None);
    }

    #[test]
    fn stop_monitoring_leaves_real_agent_work_alone() {
        let tracker = SubagentTracker::default();
        let now = SystemTime::now();
        tracker.decide("t1", &monitor_event("m1", SubagentStatus::Running), now);
        tracker.decide("t1", &subagent_event("s1", SubagentStatus::Running), now);
        tracker.decide("t1", &turn_completed(), now);
        assert_eq!(
            tracker.stop_monitoring("t1"),
            Some(PaneStatus::Working),
            "stopping the watch loops must not abandon a live subagent"
        );
    }

    // Finding 8: Stop has to survive the demux's memory. The Claude
    // translator re-stamps `task_kind` onto every later tick of a task it
    // classified, so a detached watch loop that outlives the interrupt
    // would otherwise walk straight back into the monitor set.
    #[test]
    fn a_stopped_monitor_cannot_walk_back_in_on_its_next_tick() {
        let tracker = SubagentTracker::default();
        let now = SystemTime::now();
        tracker.decide("t1", &monitor_event("m1", SubagentStatus::Running), now);
        tracker.decide("t1", &turn_completed(), now);
        assert_eq!(tracker.stop_monitoring("t1"), Some(PaneStatus::Review));

        // The surviving task keeps ticking, still labelled a monitor.
        assert_eq!(
            tracker.decide("t1", &monitor_event("m1", SubagentStatus::Running), now),
            None,
            "a stopped monitor's later ticks must not re-light the badge"
        );
        assert_eq!(
            tracker.decide("t1", &monitor_event("m1", SubagentStatus::Running), now),
            None
        );

        // A new turn is the boundary: the blocklist goes with the rest of
        // the run's state, so a monitor started by the *next* turn counts.
        tracker.decide("t1", &session_running(), now);
        tracker.decide("t1", &monitor_event("m1", SubagentStatus::Running), now);
        assert_eq!(
            tracker.decide("t1", &turn_completed(), now),
            Some(PaneStatus::Monitoring),
            "the blocklist is scoped to the run it was recorded in"
        );
    }

    #[test]
    fn stop_monitoring_blocklist_does_not_leak_the_tracker_entry() {
        let tracker = SubagentTracker::default();
        let now = SystemTime::now();
        tracker.decide("t1", &monitor_event("m1", SubagentStatus::Running), now);
        tracker.decide("t1", &turn_completed(), now);
        tracker.stop_monitoring("t1");
        assert_eq!(
            tracker.tracked_thread_count(),
            1,
            "the blocklist has to outlive the tasks it names"
        );
        // ...but only until the run boundary that supersedes it.
        tracker.decide(
            "t1",
            &ProviderRuntimeEvent::SessionStateChanged {
                thread_id: tid(),
                status: SessionStatus::Closed,
            },
            now,
        );
        assert_eq!(tracker.tracked_thread_count(), 0);
    }

    // Finding 3's invariant, stated as a test: a monitor-only thread owes
    // no Review at all, so the 600s watchdog can never force-settle it out
    // of its badge no matter how long the watch loop runs quietly.
    #[test]
    fn the_watchdog_never_force_settles_a_monitor_only_thread() {
        let tracker = SubagentTracker::default();
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        tracker.decide("t", &monitor_event("m1", SubagentStatus::Running), start);
        assert_eq!(
            tracker.decide("t", &turn_completed(), start),
            Some(PaneStatus::Monitoring)
        );
        // Hours of silence later, and after further monitor ticks.
        let much_later = start + Duration::from_secs(6 * 3600);
        tracker.decide("t", &monitor_event("m1", SubagentStatus::Running), much_later);
        assert!(
            tracker
                .take_overdue_reviews(much_later, STALL_THRESHOLD)
                .is_empty(),
            "a monitor-only thread is invisible to the sweep"
        );
        // And the badge is still the truth.
        assert_eq!(
            tracker.decide("t", &monitor_event("m1", SubagentStatus::Running), much_later),
            Some(PaneStatus::Monitoring)
        );
    }

    // Finding 3's other half: a monitor tick must not re-arm the silence
    // clock for a thread that *is* blocked by a real subagent, or a
    // chatty watch loop would keep the backstop from ever firing.
    #[test]
    fn a_monitor_tick_does_not_re_arm_the_owed_review_clock() {
        assert!(!refreshes_owed_review(&monitor_event(
            "m1",
            SubagentStatus::Running
        )));
        assert!(!refreshes_owed_review(&background_monitor_event(
            "bg",
            SubagentStatus::Running
        )));
        assert!(refreshes_owed_review(&subagent_event(
            "s1",
            SubagentStatus::Running
        )));

        let tracker = SubagentTracker::default();
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        tracker.decide("t", &subagent_event("s1", SubagentStatus::Running), start);
        tracker.decide("t", &monitor_event("m1", SubagentStatus::Running), start);
        tracker.decide("t", &turn_completed(), start);
        // The subagent goes silent while the watch loop keeps chattering.
        let overdue_at = start + STALL_THRESHOLD;
        tracker.decide("t", &monitor_event("m1", SubagentStatus::Running), overdue_at);
        assert_eq!(
            tracker.take_overdue_reviews(overdue_at, STALL_THRESHOLD),
            vec!["t".to_string()],
            "a chatty monitor must not shield a silent subagent"
        );
    }

    #[test]
    fn run_finished_covers_every_settled_status() {
        // Backs the background-agent-browser release wiring in
        // `publish_pane_status`: a run is over once the tracker settles to
        // Review (turn done, nothing left in flight), Monitoring (turn
        // done, only watch loops left) or Idle (turn done and already
        // seen, or session closed/errored). A mid-run Working or an
        // approval prompt must never release the chip out from under a
        // still-live agent.
        assert!(run_finished(PaneStatus::Review));
        assert!(run_finished(PaneStatus::Idle));
        assert!(!run_finished(PaneStatus::Working));
        assert!(!run_finished(PaneStatus::Permission));
        // A watch loop is not the run: letting it hold the browser session
        // would put "is the run over?" back at the mercy of a task that
        // may never terminate.
        assert!(run_finished(PaneStatus::Monitoring));
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
        assert!(st.review_pending(), "a Review is now owed");
        assert!(st.tasks.contains_key("s1"));
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
        assert!(st.review_pending());
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
                    status: SessionStatus::Error {
                        message: "boom".into()
                    },
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
            tracker.decide("t", &subagent_event("s1", SubagentStatus::Running), SystemTime::now()),
            None
        );
        assert_eq!(
            tracker.decide("t", &turn_completed(), SystemTime::now()),
            Some(PaneStatus::Working)
        );
        assert_eq!(
            tracker.decide(
                "t",
                &subagent_event("s1", SubagentStatus::Completed),
                SystemTime::now()
            ),
            Some(PaneStatus::Review)
        );
        // Entry is dropped; a stray duplicate terminal is a harmless None.
        assert_eq!(
            tracker.decide(
                "t",
                &subagent_event("s1", SubagentStatus::Completed),
                SystemTime::now()
            ),
            None
        );
        // clear_thread is idempotent on an already-gone thread.
        tracker.clear_thread("t");
        tracker.clear_thread("does-not-exist");
    }

    // ── Background tasks must not block run settlement ──

    /// A `SubagentUpdated` for a provider **background task** (Claude's
    /// `Bash { run_in_background: true }` shape), which never emits a
    /// terminal update when the job outlives the turn.
    fn background_task_event(id: &str, status: SubagentStatus) -> ProviderRuntimeEvent {
        ProviderRuntimeEvent::SubagentUpdated {
            thread_id: tid(),
            subagent: SubagentSnapshot {
                subagent_id: id.into(),
                status,
                background_task: true,
                ..Default::default()
            },
        }
    }

    // The root-cause regression: a turn that launched a background shell
    // command (a dev server that never exits, so no terminal
    // `task_notification` ever arrives) must still settle to `Review` on
    // `TurnCompleted` — no owed Review, no stuck "Working", so the
    // background-browser release downstream actually fires.
    #[test]
    fn background_task_does_not_defer_review_on_turn_completed() {
        let mut st = ThreadSubagentState::default();
        let bg_running = background_task_event("toolu_bash", SubagentStatus::Running);
        assert_eq!(
            map_event_to_pane_status(&bg_running, &mut st),
            None,
            "a background task never moves the dot on its own"
        );
        assert!(
            st.tasks.is_empty(),
            "a background task is never tracked as a live subagent"
        );
        assert_eq!(
            map_event_to_pane_status(&turn_completed(), &mut st),
            Some(PaneStatus::Review),
            "the turn settles even though the background job is still alive"
        );
        assert!(!st.review_pending(), "no Review is owed");
        assert!(st.is_clear());
    }

    // A background-task progress tick arriving AFTER the run settled must
    // not resurrect `Working` — that is what kept the sidebar spinning and
    // the browser chip on `LIVE` overnight.
    #[test]
    fn background_task_tick_after_settle_does_not_resurrect_working() {
        let bg_running = background_task_event("toolu_bash", SubagentStatus::Running);
        let mut st = ThreadSubagentState::default();
        map_event_to_pane_status(&bg_running, &mut st);
        map_event_to_pane_status(&turn_completed(), &mut st);
        assert_eq!(map_event_to_pane_status(&bg_running, &mut st), None);

        // Even with a genuine subagent owing a Review, a background tick
        // must stay silent rather than re-publishing Working.
        let mut st = ThreadSubagentState::default();
        map_event_to_pane_status(&subagent_event("s1", SubagentStatus::Running), &mut st);
        map_event_to_pane_status(&turn_completed(), &mut st);
        assert!(st.review_pending());
        assert_eq!(map_event_to_pane_status(&bg_running, &mut st), None);
        assert!(st.review_pending(), "the real subagent still owes its Review");
    }

    // Defensive path: an id inserted by an earlier unflagged snapshot (an
    // older build, a provider that learns the shape late) is dropped from
    // the running-set the moment a flagged snapshot for it arrives.
    #[test]
    fn flagged_snapshot_evicts_a_previously_unflagged_background_id() {
        let mut st = ThreadSubagentState::default();
        map_event_to_pane_status(&subagent_event("toolu_bash", SubagentStatus::Running), &mut st);
        assert!(st.tasks.contains_key("toolu_bash"));
        let bg_running = background_task_event("toolu_bash", SubagentStatus::Running);
        map_event_to_pane_status(&bg_running, &mut st);
        assert!(st.tasks.is_empty());
        assert_eq!(
            map_event_to_pane_status(&turn_completed(), &mut st),
            Some(PaneStatus::Review)
        );
    }

    // The deliberate async-subagent flow must be untouched: a real
    // (unflagged) subagent still holds `Working` past `TurnCompleted` and
    // its terminal snapshot still publishes the owed `Review`.
    #[test]
    fn real_subagent_still_defers_review_until_it_finishes() {
        let mut st = ThreadSubagentState::default();
        map_event_to_pane_status(&subagent_event("s1", SubagentStatus::Running), &mut st);
        assert_eq!(
            map_event_to_pane_status(&turn_completed(), &mut st),
            Some(PaneStatus::Working)
        );
        assert!(st.review_pending());
        assert_eq!(
            map_event_to_pane_status(&subagent_event("s1", SubagentStatus::Completed), &mut st),
            Some(PaneStatus::Review)
        );
        assert!(st.is_clear());
    }

    // ── Watchdog force-settle of an owed Review ──

    #[test]
    fn select_overdue_review_only_fires_on_a_silent_owed_review() {
        let now = SystemTime::now();
        let stale = now - STALL_THRESHOLD - Duration::from_secs(1);

        let agent_task = || {
            [("s1".to_string(), TaskClass::Agent)]
                .into_iter()
                .collect::<std::collections::BTreeMap<_, _>>()
        };

        // Nothing owed (the turn is still in flight) → never overdue,
        // however old the state is.
        let mid_turn = ThreadSubagentState {
            tasks: agent_task(),
            turn_settled: false,
            review_owed_since: Some(stale),
            ..Default::default()
        };
        assert!(!select_overdue_review(&mid_turn, now, STALL_THRESHOLD));

        // Owed but still fresh → the deferred Review is allowed to stand.
        let fresh = ThreadSubagentState {
            tasks: agent_task(),
            turn_settled: true,
            review_owed_since: Some(now),
            ..Default::default()
        };
        assert!(!select_overdue_review(&fresh, now, STALL_THRESHOLD));

        // Owed and silent past the threshold → force-settle.
        let overdue = ThreadSubagentState {
            review_owed_since: Some(stale),
            ..fresh.clone()
        };
        assert!(select_overdue_review(&overdue, now, STALL_THRESHOLD));

        // Nothing *blocking* the settle → never overdue, whatever the
        // clock says. A monitor-only thread settled at `TurnCompleted`
        // and owes no Review, so the sweep can never take its badge.
        let monitor_only = ThreadSubagentState {
            tasks: [("m1".to_string(), TaskClass::Monitor)]
                .into_iter()
                .collect(),
            turn_settled: true,
            review_owed_since: Some(stale),
            ..Default::default()
        };
        assert!(!select_overdue_review(&monitor_only, now, STALL_THRESHOLD));
    }

    #[test]
    fn take_overdue_reviews_settles_once_and_tombstones_the_entry() {
        let tracker = SubagentTracker::default();
        let start = SystemTime::now();
        tracker.decide("t", &subagent_event("s1", SubagentStatus::Running), start);
        assert_eq!(
            tracker.decide("t", &turn_completed(), start),
            Some(PaneStatus::Working),
            "Review deferred behind a subagent that never goes terminal"
        );

        // Not yet silent long enough.
        assert!(tracker
            .take_overdue_reviews(start + Duration::from_secs(60), STALL_THRESHOLD)
            .is_empty());

        let later = start + STALL_THRESHOLD + Duration::from_secs(1);
        assert_eq!(
            tracker.take_overdue_reviews(later, STALL_THRESHOLD),
            vec!["t".to_string()]
        );
        // Take-once: the owed Review was cleared, so a later sweep
        // re-settles nothing...
        assert!(tracker
            .take_overdue_reviews(later + STALL_THRESHOLD, STALL_THRESHOLD)
            .is_empty());
        // ...but the entry itself survives as a tombstone, because `s1` may
        // still be alive and its terminal snapshot still has to land
        // somewhere that knows a Review was already published.
        assert_eq!(tracker.tracked_thread_count(), 1);
    }

    // Finding (b): a forced settle must not strand tracker state. The late
    // real completion drains the tombstone, publishes nothing, and leaves
    // the map empty — as opposed to dropping the entry at settle time,
    // where the same snapshot would recreate a `review_pending: false` husk
    // that nothing ever collects.
    #[test]
    fn a_late_completion_after_a_forced_settle_is_silent_and_leaks_nothing() {
        let tracker = SubagentTracker::default();
        let start = SystemTime::now();
        tracker.decide("t", &subagent_event("s1", SubagentStatus::Running), start);
        tracker.decide("t", &turn_completed(), start);
        let settled_at = start + STALL_THRESHOLD + Duration::from_secs(1);
        assert_eq!(
            tracker.take_overdue_reviews(settled_at, STALL_THRESHOLD),
            vec!["t".to_string()]
        );

        // The subagent was alive all along — just quiet inside one long
        // tool call — and reports again after the forced settle.
        assert_eq!(
            tracker.decide(
                "t",
                &subagent_event("s1", SubagentStatus::Running),
                settled_at + Duration::from_secs(5)
            ),
            None,
            "a post-settle progress tick must not resurrect Working"
        );
        assert_eq!(
            tracker.decide(
                "t",
                &subagent_event("s1", SubagentStatus::Completed),
                settled_at + Duration::from_secs(10)
            ),
            None,
            "the Review was already published; do not re-announce it"
        );
        assert_eq!(
            tracker.tracked_thread_count(),
            0,
            "the tombstone is collected once the last tracked id drains"
        );
    }

    // A forced settle on a thread whose next turn simply starts again must
    // behave like any other thread: the tombstone is part of the state a
    // new turn resets, so turn 2 settles normally.
    #[test]
    fn a_new_turn_clears_the_forced_settle_tombstone() {
        let tracker = SubagentTracker::default();
        let start = SystemTime::now();
        tracker.decide("t", &subagent_event("s1", SubagentStatus::Running), start);
        tracker.decide("t", &turn_completed(), start);
        let settled_at = start + STALL_THRESHOLD + Duration::from_secs(1);
        tracker.take_overdue_reviews(settled_at, STALL_THRESHOLD);

        assert_eq!(
            tracker.decide("t", &session_running(), settled_at + Duration::from_secs(1)),
            Some(PaneStatus::Working)
        );
        assert_eq!(
            tracker.decide("t", &turn_completed(), settled_at + Duration::from_secs(2)),
            Some(PaneStatus::Review),
            "turn 2 settles through the ordinary path"
        );
        assert_eq!(tracker.tracked_thread_count(), 0);
    }

    // Finding (a), the destructive half: the watchdog's trigger is silence,
    // which a live subagent inside one long quiet tool call also produces.
    // Publishing a settled dot too early is repainted by the next real
    // event; tearing down the browser session that subagent is driving is
    // not recoverable, so only a provider event may do it.
    #[test]
    fn a_forced_settle_never_releases_the_detached_browser() {
        for status in [PaneStatus::Review, PaneStatus::Idle] {
            assert!(
                should_release_browser(SettleOrigin::ProviderEvent, status.clone()),
                "a real terminal transition still releases"
            );
            assert!(
                !should_release_browser(SettleOrigin::ForcedBackstop, status),
                "the watchdog's backstop must never tear down a live browser"
            );
        }
        // Mid-run statuses are unchanged from either origin.
        for origin in [SettleOrigin::ProviderEvent, SettleOrigin::ForcedBackstop] {
            assert!(!should_release_browser(origin, PaneStatus::Working));
            assert!(!should_release_browser(origin, PaneStatus::Permission));
        }
    }

    // Real post-turn subagent activity refreshes the silence clock, so a
    // busy async subagent is never force-settled out from under a live run.
    #[test]
    fn real_subagent_activity_defers_the_forced_settle() {
        let tracker = SubagentTracker::default();
        let start = SystemTime::now();
        tracker.decide("t", &subagent_event("s1", SubagentStatus::Running), start);
        tracker.decide("t", &turn_completed(), start);

        let almost = start + STALL_THRESHOLD - Duration::from_secs(1);
        tracker.decide("t", &subagent_event("s1", SubagentStatus::Running), almost);
        assert!(
            tracker
                .take_overdue_reviews(almost + Duration::from_secs(2), STALL_THRESHOLD)
                .is_empty(),
            "a live subagent snapshot re-arms the clock"
        );

        // A background-task tick, by contrast, is not activity — the clock
        // keeps running from the last real signal.
        tracker.decide(
            "t",
            &background_task_event("toolu_bash", SubagentStatus::Running),
            almost + Duration::from_secs(2),
        );
        assert_eq!(
            tracker.take_overdue_reviews(almost + STALL_THRESHOLD, STALL_THRESHOLD),
            vec!["t".to_string()]
        );
    }

    fn session_running() -> ProviderRuntimeEvent {
        ProviderRuntimeEvent::SessionStateChanged {
            thread_id: tid(),
            status: SessionStatus::Running {
                active_turn: turn(),
            },
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
        assert!(st.review_pending());
        assert!(st.tasks.contains_key("bg"));

        // Turn 2 starts: the new-turn reset drains the stale tracking.
        assert_eq!(
            map_event_to_pane_status(&session_running(), &mut st),
            Some(PaneStatus::Working)
        );
        assert!(
            st.is_clear(),
            "new turn resets both running and review_pending"
        );

        // Turn 2 finishes with nothing tracked → Review fires normally.
        assert_eq!(
            map_event_to_pane_status(&turn_completed(), &mut st),
            Some(PaneStatus::Review),
            "no stale id suppresses the second turn's Review"
        );
    }

    // Same regression via the authoritative anchor: the send-message
    // command starts a fresh tracker turn. Stale agent work is discarded.
    #[test]
    fn send_command_turn_boundary_resets_agent_tracking_across_turns() {
        let tracker = SubagentTracker::default();
        // Turn 1: subagent outlives the turn, Review deferred.
        tracker.decide(
            "t",
            &subagent_event("bg", SubagentStatus::Running),
            SystemTime::now(),
        );
        assert_eq!(
            tracker.decide("t", &turn_completed(), SystemTime::now()),
            Some(PaneStatus::Working)
        );

        // agent_chat_send_turn starts a new tracker turn before turn 2.
        tracker.begin_turn("t");

        // Turn 2 completes clean → Review, not a stuck Working.
        assert_eq!(
            tracker.decide("t", &turn_completed(), SystemTime::now()),
            Some(PaneStatus::Review)
        );
    }

    // Production regression: Claude's `TaskOutput` can prove the original
    // monitor is still running without emitting another `task_progress`
    // snapshot. The command-path turn boundary therefore has to retain the
    // known monitor by itself; waiting for a provider tick loses the badge.
    // The tracker is provider-neutral, so this also covers any future Codex or
    // OpenCode adapter that classifies a `SubagentSnapshot` as a monitor.
    #[test]
    fn send_command_turn_boundary_preserves_monitor_without_a_new_snapshot() {
        let tracker = SubagentTracker::default();
        let now = SystemTime::now();
        tracker.decide("t", &monitor_event("watch", SubagentStatus::Running), now);
        assert_eq!(
            tracker.decide("t", &turn_completed(), now),
            Some(PaneStatus::Monitoring)
        );

        // User sends a follow-up. No SubagentUpdated event arrives for the
        // still-running watch task during this turn.
        tracker.begin_turn("t");
        assert_eq!(tracker.tracked_thread_count(), 1);
        assert_eq!(
            tracker.decide("t", &turn_completed(), now),
            Some(PaneStatus::Monitoring),
            "the monitor reappears when the follow-up settles"
        );
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
        assert!(st.review_pending());

        // A late parent-scoped item (subagent_id == None) arrives after the
        // deferred TurnCompleted. It keeps Working but leaves the owed
        // Review intact.
        assert_eq!(
            map_event_to_pane_status(
                &ProviderRuntimeEvent::ItemCompleted {
                    thread_id: tid(),
                    turn_id: turn(),
                    item: CompletedItem::AssistantText {
                        text: "final".into()
                    },
                    subagent_id: None,
                },
                &mut st
            ),
            Some(PaneStatus::Working)
        );
        assert!(
            st.review_pending(),
            "parent-scoped output must not drop the owed Review"
        );

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

    fn portable_skill() -> crate::skills::ResolvedSkillInvocation {
        crate::skills::ResolvedSkillInvocation {
            skill_id: "skill-1".into(),
            name: "deploy".into(),
            source_provider: crate::skills::SkillProvider::Claude,
            source_scope: crate::skills::SkillScope::User,
            base_dir: Some("/skills/deploy".into()),
            path: Some("/skills/deploy/SKILL.md".into()),
            body: Some("Deploy carefully.".into()),
            invocation: crate::skills::SkillInvocationKind::PromptPrefix,
        }
    }

    #[test]
    fn portable_skill_envelope_preserves_base_directory() {
        let rendered = render_skill_invocations(
            "ship it",
            Some("ship it"),
            ProviderKind::OpenCode,
            &[portable_skill()],
        )
        .unwrap();
        assert!(rendered.contains("base-dir=\"/skills/deploy\""));
        assert!(rendered.contains("Deploy carefully."));
        assert!(rendered.ends_with("ship it"));
    }

    #[test]
    fn portable_skill_envelope_escapes_attribute_values() {
        let mut skill = portable_skill();
        skill.name = "deploy&verify".into();
        skill.base_dir = Some("/skills/a\"b".into());
        let rendered =
            render_skill_invocations("ship", Some("ship"), ProviderKind::OpenCode, &[skill])
                .unwrap();
        assert!(rendered.contains("name=\"deploy&amp;verify\""));
        assert!(rendered.contains("base-dir=\"/skills/a&quot;b\""));
    }

    #[test]
    fn native_claude_skill_keeps_native_command_semantics() {
        let mut skill = portable_skill();
        skill.invocation = crate::skills::SkillInvocationKind::NativeCommand;
        let rendered =
            render_skill_invocations("release", Some("release"), ProviderKind::Claude, &[skill])
                .unwrap();
        assert_eq!(rendered, "/deploy release");
    }

    #[test]
    fn native_claude_skill_with_wrappers_uses_portable_envelope() {
        let mut skill = portable_skill();
        skill.invocation = crate::skills::SkillInvocationKind::NativeCommand;
        let rendered = render_skill_invocations(
            "Plan instructions.\n\nrelease",
            Some("release"),
            ProviderKind::Claude,
            &[skill],
        )
        .unwrap();
        assert!(rendered.starts_with("<codemux-skill"));
        assert!(rendered.ends_with("Plan instructions.\n\nrelease"));
        assert!(!rendered.starts_with("/deploy"));
    }

    #[test]
    fn native_claude_skill_without_original_text_uses_portable_envelope() {
        let mut skill = portable_skill();
        skill.invocation = crate::skills::SkillInvocationKind::NativeCommand;
        let rendered =
            render_skill_invocations("release", None, ProviderKind::Claude, &[skill]).unwrap();
        assert!(rendered.starts_with("<codemux-skill"));
        assert!(rendered.ends_with("release"));
    }

    #[test]
    fn codex_skill_item_does_not_duplicate_body_in_text() {
        let mut skill = portable_skill();
        skill.invocation = crate::skills::SkillInvocationKind::CodexSkillItem;
        let rendered =
            render_skill_invocations("review", Some("review"), ProviderKind::Codex, &[skill])
                .unwrap();
        assert_eq!(rendered, "review");
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
