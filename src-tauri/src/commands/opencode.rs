//! Tauri command surface for the OpenCode driver — Step 12 Stages 1-2.
//!
//! Three commands today:
//!
//! * [`opencode_check_availability`] (Stage 1) — wraps
//!   [`crate::agent_provider::opencode::check_opencode_availability`]
//!   so the frontend can render a "OpenCode not installed" empty
//!   state without owning subprocess plumbing itself. The frontend
//!   also drives whether to probe a server URL — passing `None` skips
//!   the network round-trip, which is the right call at app boot.
//! * [`opencode_ping`] (Stage 1) — sends a `GET` against a
//!   caller-supplied `base_url` and reports success/failure. Used by
//!   future settings surfaces that need to validate "yes my custom
//!   OpenCode endpoint is reachable" before the user commits a config
//!   change.
//! * [`opencode_list_models`] (Stage 2) — spins up the lazily-managed
//!   `opencode serve` child via [`OpenCodeServerManager`], hits
//!   `GET /provider`, and returns the flattened
//!   `Vec<OpenCodeProviderEntry>`. No UI consumer yet (Stage 3 wires
//!   it through `ChatModelInfo`); the command exists so the model
//!   harvest is independently testable from devtools / e2e probes.
//!
//! All three are intentionally feature-flag-agnostic: the OpenCode
//! integration is gated by the `enable_agent_chat` flag at the picker
//! level, but discovery + harvest are pure reads that the picker needs
//! even when it's about to render an empty/disabled state.

use tauri::State;

use crate::agent_provider::opencode::{
    check_opencode_availability, OpenCodeAvailability, OpenCodeClient, OpenCodeClientConfig,
    OpenCodeProviderEntry, OpenCodeServerManager,
};

/// Probe whether OpenCode is usable on this machine. See
/// [`OpenCodeAvailability`] for the field-by-field semantics.
///
/// `server_url` is forwarded to the discovery layer verbatim; the
/// frontend can pass `None` to skip the HTTP probe, which is what app
/// boot does — the only Stage 1 caller that supplies a value is the
/// (future) settings panel.
#[tauri::command]
pub async fn opencode_check_availability(
    server_url: Option<String>,
) -> Result<OpenCodeAvailability, String> {
    Ok(check_opencode_availability(server_url).await)
}

/// Round-trip a `GET /` against `base_url` and report success.
///
/// Treats any HTTP response (including 4xx/5xx) as "server reachable"
/// — the discovery layer's auth-agnostic probe semantics carry over
/// here. The frontend wraps the resulting `Err` string in a
/// user-visible toast; the strings come from
/// [`crate::agent_provider::opencode::client::format_request_error`]
/// so `connect_failed` / `request_timed_out` / `http_status_<code>`
/// remain pinable across versions.
///
/// `server_password` is forwarded as the HTTP Basic password (username
/// is always `"opencode"` per OpenCode's protocol). Passing `None` is
/// fine for the no-password local-loopback case.
#[tauri::command]
pub async fn opencode_ping(
    base_url: String,
    server_password: Option<String>,
) -> Result<(), String> {
    let mut config = OpenCodeClientConfig::new(base_url);
    config.server_password = server_password;
    let client = OpenCodeClient::new(config)?;
    client.ping().await
}

/// Fetch the full provider catalogue from the running OpenCode
/// server. Spawns the server lazily on the first call (per
/// [`OpenCodeServerManager`]); subsequent calls reuse the same
/// child.
///
/// Returns the flattened
/// `Vec<OpenCodeProviderEntry>` (~116 entries / ~4 354 models on a
/// fully-populated dev box). Stage 3 maps each entry into
/// `ChatModelInfo`; Stage 1/2 just guarantee the data is reachable.
///
/// Error vocabulary (stable across versions):
///
/// * `"opencode_not_installed"` — `opencode` not on PATH.
/// * `"spawn_failed: …"` / `"ready_banner_missing"` /
///   `"ready_timeout_after_<n>ms"` — server failed to come up.
/// * `"connect_failed"` / `"request_timed_out"` /
///   `"http_status_<n>"` / `"parse_error: …"` /
///   `"request_error: …"` — HTTP-level failures from
///   [`OpenCodeClient::list_models`].
#[tauri::command]
pub async fn opencode_list_models(
    manager: State<'_, OpenCodeServerManager>,
) -> Result<Vec<OpenCodeProviderEntry>, String> {
    let handle = manager.ensure_running().await?;
    let mut config = OpenCodeClientConfig::new(handle.base_url);
    config.server_password = Some(handle.server_password);
    let client = OpenCodeClient::new(config)?;
    client.list_models().await
}
