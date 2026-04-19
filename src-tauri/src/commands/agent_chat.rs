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

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::agent_provider::{
    AgentProvider, ApprovalDecision, ProviderError, ProviderKind, ProviderRuntimeEvent,
    RequestId, SendTurnInput, SerializableProviderError, StartSessionInput, ThreadId, TurnId,
};
use crate::observability::ObservabilityStore;
use crate::state::AppStateStore;

/// Event name emitted by the backend whenever a provider produces a
/// runtime event. The frontend's `useAgentChatEvents` hook subscribes
/// to this channel and filters by `thread_id`.
pub const AGENT_CHAT_EVENT: &str = "agent_chat_event";

/// Payload emitted on the [`AGENT_CHAT_EVENT`] Tauri channel.
///
/// Carries the originating thread id alongside the raw canonical
/// event so subscribers can filter without re-parsing the payload.
/// Events that are not scoped to a single thread (global
/// `RuntimeWarning`s) are emitted with an empty `ThreadId`.
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
#[derive(Default)]
pub struct ProviderRegistry {
    claude: tokio::sync::RwLock<Option<Arc<dyn AgentProvider>>>,
    codex: tokio::sync::RwLock<Option<Arc<dyn AgentProvider>>>,
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

    /// Look up the provider for a given kind. Returns `None` when no
    /// provider has been registered — commands must return a clean
    /// error in that case instead of panicking.
    pub async fn get(&self, kind: ProviderKind) -> Option<Arc<dyn AgentProvider>> {
        match kind {
            ProviderKind::Claude => self.claude.read().await.clone(),
            ProviderKind::Codex => self.codex.read().await.clone(),
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
#[tauri::command]
pub fn agent_chat_create_pane(
    app: AppHandle,
    state: State<'_, AppStateStore>,
    observability: State<'_, ObservabilityStore>,
    workspace_id: String,
    provider: Option<ProviderKind>,
    cwd: Option<String>,
) -> Result<String, String> {
    feature_flag_on(&observability)?;
    let pane_id = state.create_agent_chat_pane(&workspace_id, provider, cwd)?;
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
    // close_pane errors when the pane id is unknown — treat that as a
    // no-op to keep the command idempotent.
    let _ = state.close_pane(&pane_id);
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
    let pane_id = state.create_agent_chat_pane(&workspace_id, None, None)?;
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
/// resolve it without re-consulting the provider.
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
    let session = impl_.start_session(input).await.map_err(provider_err)?;
    let state: State<'_, AppStateStore> = app.state();
    state.set_agent_chat_thread_id(&pane_id, Some(session.thread_id.0.clone()));
    crate::state::emit_app_state(&app);
    Ok(session.thread_id)
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
    let turn = impl_.send_turn(input).await.map_err(provider_err)?;
    Ok(turn.turn_id)
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

// ── Event bridge ──────────────────────────────────────────────────────

/// Start the provider-event forwarding tasks.
///
/// Spawns one Tokio task per registered provider. Each task consumes
/// the provider's canonical event stream and re-emits each event on
/// the [`AGENT_CHAT_EVENT`] Tauri channel wrapped in an
/// [`AgentChatEventPayload`]. When a provider's stream ends (on
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

/// Emit a single provider event to the frontend.
///
/// Extracted so tests can exercise the translation without spinning a
/// Tokio task or a real provider. Also used as the inner loop of
/// [`spawn_event_bridge`].
pub fn forward_event<R: Runtime>(app: &AppHandle<R>, event: ProviderRuntimeEvent) {
    let thread_id = thread_id_for_event(&event)
        // Events without a thread_id (e.g. global RuntimeWarning) are
        // forwarded with an empty ThreadId so the frontend at least
        // sees them; a richer global-warning channel is a follow-up.
        .unwrap_or_else(|| ThreadId(String::new()));
    let payload = AgentChatEventPayload { thread_id, event };
    if let Err(error) = app.emit(AGENT_CHAT_EVENT, &payload) {
        eprintln!("[codemux::agent_chat] Failed to emit {AGENT_CHAT_EVENT}: {error}");
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
        | ProviderRuntimeEvent::SessionStateChanged { thread_id, .. } => Some(thread_id.clone()),
        ProviderRuntimeEvent::RuntimeWarning { thread_id, .. } => thread_id.clone(),
    }
}
