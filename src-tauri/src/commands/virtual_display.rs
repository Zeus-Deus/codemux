//! Tauri commands for the Phase 2 virtual display feature.
//!
//! The frontend calls `get_workspace_virtual_display(workspace_id)` to
//! discover whether a workspace has a hidden display and, if so, where the
//! x11vnc stream is listening. A future "watch the agent" pane uses the
//! returned port to open a noVNC session in a Codemux browser surface.

use serde::Serialize;
use tauri::State;

use crate::execution::virtual_display::VirtualDisplayManager;

/// Shape returned to the frontend. All fields are optional so the TS side
/// can render a disabled state when a workspace has no display at all.
#[derive(Debug, Clone, Serialize)]
pub struct VirtualDisplayInfo {
    /// X display identifier, e.g. `":1042"`. `None` when no display is
    /// currently acquired.
    pub display: Option<String>,
    /// Localhost TCP port where `x11vnc` is listening. `None` if the user
    /// didn't opt into VNC or `x11vnc` isn't installed.
    pub vnc_port: Option<u16>,
    /// Plain-text VNC password the viewer component should send. `None` when
    /// VNC isn't active. Phase 2.5: we ship a random per-display token so
    /// "watch the agent" doesn't rely on `-nopw`.
    pub vnc_password: Option<String>,
    /// True if `is_supported()` would let a cold acquire succeed on this
    /// host (Linux + Xvfb on PATH). The frontend uses this to hide the
    /// "watch" button on platforms where display isolation isn't available.
    pub supported: bool,
}

/// Look up the virtual display currently allocated for a workspace.
///
/// Returns `VirtualDisplayInfo { display: None, vnc_port: None, supported }`
/// when the workspace has no display — which is the normal state until the
/// user's first agent spawn in that workspace with `virtual_display: true`.
#[tauri::command]
pub fn get_workspace_virtual_display(
    workspace_id: String,
    manager: State<'_, VirtualDisplayManager>,
) -> VirtualDisplayInfo {
    let supported = VirtualDisplayManager::is_supported();
    match manager.env_for_workspace(&workspace_id) {
        Some(env) => VirtualDisplayInfo {
            display: Some(env.display),
            vnc_port: env.vnc_port,
            vnc_password: env.vnc_password,
            supported,
        },
        None => VirtualDisplayInfo {
            display: None,
            vnc_port: None,
            vnc_password: None,
            supported,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_serializes_with_snake_case() {
        let info = VirtualDisplayInfo {
            display: Some(":1042".to_string()),
            vnc_port: Some(5910),
            vnc_password: Some("deadbeef".to_string()),
            supported: true,
        };
        let json = serde_json::to_string(&info).unwrap();
        // Field names serialize verbatim (no rename_all) — keep in sync with
        // the TypeScript interface.
        assert!(json.contains("\"display\":\":1042\""));
        assert!(json.contains("\"vnc_port\":5910"));
        assert!(json.contains("\"vnc_password\":\"deadbeef\""));
        assert!(json.contains("\"supported\":true"));
    }
}
