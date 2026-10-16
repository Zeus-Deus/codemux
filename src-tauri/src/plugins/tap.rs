//! The plugin chat event tap: every provider event, for every thread, to
//! `host.chat.onEvent` in the main window.
//!
//! Off unless a plugin listens. The frontend turns it on with the first
//! subscriber and off with the last (`plugins_set_event_tap`), and a main
//! page reload turns it off because that page's listeners are gone. While
//! off, `forward_event` pays one relaxed atomic load per event and emits
//! nothing.

use crate::agent_provider::{ProviderRuntimeEvent, ThreadId};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Runtime};

/// Event name the main window listens on.
pub const PLUGIN_AGENT_EVENT: &str = "plugin_agent_event";

static TAP_ON: AtomicBool = AtomicBool::new(false);

pub fn set_enabled(on: bool) {
    TAP_ON.store(on, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    TAP_ON.load(Ordering::Relaxed)
}

#[derive(Serialize)]
pub struct PluginAgentEventPayload<'a> {
    pub thread_id: &'a str,
    pub event: &'a ProviderRuntimeEvent,
}

/// Build the payload, or `None` while the tap is off. Split from [`emit`] so
/// the on/off rule is unit-testable without an app.
pub fn payload<'a>(
    thread_id: Option<&'a ThreadId>,
    event: &'a ProviderRuntimeEvent,
) -> Option<PluginAgentEventPayload<'a>> {
    if !enabled() {
        return None;
    }
    Some(PluginAgentEventPayload {
        thread_id: thread_id.map(|t| t.0.as_str()).unwrap_or(""),
        event,
    })
}

/// Called from `forward_event` before any branch can swallow the event, so
/// `UsageRecorded` and `PlanUsageUpdated` reach plugins too.
pub fn emit<R: Runtime>(app: &AppHandle<R>, event: &ProviderRuntimeEvent) {
    if !enabled() {
        return;
    }
    let thread_id = crate::commands::agent_chat::thread_id_for_event(event);
    if let Some(payload) = payload(thread_id.as_ref(), event) {
        if let Err(error) = app.emit_to("main", PLUGIN_AGENT_EVENT, &payload) {
            eprintln!("[codemux::plugins] failed to emit {PLUGIN_AGENT_EVENT}: {error}");
        }
    }
}
