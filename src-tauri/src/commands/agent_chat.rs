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

use tauri::{AppHandle, State};

use crate::agent_provider::ProviderKind;
use crate::observability::ObservabilityStore;
use crate::state::AppStateStore;

/// Error string returned when a command runs with
/// `enable_agent_chat` off.
pub const FEATURE_DISABLED_ERROR: &str = "feature_disabled: enable_agent_chat is off";

pub(crate) fn feature_flag_on(store: &ObservabilityStore) -> Result<(), String> {
    if store.agent_chat_enabled() {
        Ok(())
    } else {
        Err(FEATURE_DISABLED_ERROR.to_string())
    }
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
