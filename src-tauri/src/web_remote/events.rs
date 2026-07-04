//! Global event fan-out for the web-remote server.
//!
//! Browsers subscribe to app events with `{"t":"listen","event":…}` and
//! unsubscribe with `unlisten`. The hub multiplexes all subscribers onto
//! a single `AppHandle::listen_any` registration **per event name**,
//! reference-counted by subscriber:
//!
//! - first subscriber to an event name registers the `listen_any` handler,
//! - the handler forwards every emission verbatim as
//!   `{"t":"event","event":…,"payload":<json>}` to each current subscriber,
//! - the last subscriber to leave (or disconnect) unregisters it.
//!
//! This keeps exactly one desktop-side listener alive per active event
//! regardless of how many browsers are watching it, and none once no one
//! is — the desktop's own event bus sees no extra churn.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::ws::Message;
use serde_json::{json, Value};
use tauri::{AppHandle, Listener};

use super::server::OutboundTx;

struct EventEntry {
    /// The single `listen_any` registration backing this event name.
    handler_id: tauri::EventId,
    /// Current subscribers, keyed by connection id.
    subscribers: HashMap<u64, OutboundTx>,
}

/// Per-event subscription registry. Cloneable (shares one inner map) so the
/// `listen_any` handler closure can reach back in to fan a payload out.
#[derive(Clone, Default)]
pub struct EventHub {
    inner: Arc<Mutex<HashMap<String, EventEntry>>>,
}

impl EventHub {
    /// Subscribe `conn_id` to `event`, registering the backing desktop
    /// listener if this is the first subscriber for that event name.
    pub fn subscribe(&self, app: &AppHandle, conn_id: u64, event: &str, out: OutboundTx) {
        let mut map = self.inner.lock().unwrap();
        let entry = map.entry(event.to_string()).or_insert_with(|| {
            let inner = self.inner.clone();
            let event_name = event.to_string();
            let handler_id = app.listen_any(event.to_string(), move |ev| {
                fan_out(&inner, &event_name, ev.payload());
            });
            EventEntry {
                handler_id,
                subscribers: HashMap::new(),
            }
        });
        entry.subscribers.insert(conn_id, out);
    }

    /// Unsubscribe `conn_id` from `event`; tear the desktop listener down
    /// when the last subscriber leaves.
    pub fn unsubscribe(&self, app: &AppHandle, conn_id: u64, event: &str) {
        let mut map = self.inner.lock().unwrap();
        if let Some(entry) = map.get_mut(event) {
            entry.subscribers.remove(&conn_id);
            if entry.subscribers.is_empty() {
                app.unlisten(entry.handler_id);
                map.remove(event);
            }
        }
    }

    /// Drop every subscription held by a connection (WS closed). Any event
    /// left with no subscribers has its desktop listener removed.
    pub fn remove_conn(&self, app: &AppHandle, conn_id: u64) {
        let mut map = self.inner.lock().unwrap();
        let mut emptied = Vec::new();
        for (name, entry) in map.iter_mut() {
            if entry.subscribers.remove(&conn_id).is_some() && entry.subscribers.is_empty() {
                app.unlisten(entry.handler_id);
                emptied.push(name.clone());
            }
        }
        for name in emptied {
            map.remove(&name);
        }
    }

    /// Live subscriber count for an event (test/introspection).
    #[cfg(test)]
    pub fn subscriber_count(&self, event: &str) -> usize {
        self.inner
            .lock()
            .unwrap()
            .get(event)
            .map(|e| e.subscribers.len())
            .unwrap_or(0)
    }
}

/// Forward one emission to every current subscriber of `event`. The
/// payload string is the JSON the emitter serialised; we re-embed it
/// verbatim (parsed back to a value so it nests, not double-encodes).
fn fan_out(inner: &Arc<Mutex<HashMap<String, EventEntry>>>, event: &str, payload: &str) {
    let data: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
    let frame = json!({ "t": "event", "event": event, "payload": data }).to_string();
    let map = inner.lock().unwrap();
    if let Some(entry) = map.get(event) {
        for out in entry.subscribers.values() {
            let _ = out.send(Message::Text(frame.clone()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::sync::mpsc;

    // fan_out is exercised directly here; the full listen_any registration
    // path needs a live AppHandle and is covered by the Stage-4 e2e drive.
    #[test]
    fn fan_out_delivers_event_frame_to_all_subscribers() {
        let inner: Arc<Mutex<HashMap<String, EventEntry>>> = Arc::new(Mutex::new(HashMap::new()));
        let (tx1, mut rx1) = mpsc::unbounded_channel();
        let (tx2, mut rx2) = mpsc::unbounded_channel();
        {
            let mut map = inner.lock().unwrap();
            let mut subs = HashMap::new();
            subs.insert(1u64, tx1);
            subs.insert(2u64, tx2);
            map.insert(
                "app-state".to_string(),
                EventEntry {
                    handler_id: 0,
                    subscribers: subs,
                },
            );
        }

        fan_out(&inner, "app-state", "{\"count\":3}");

        for rx in [&mut rx1, &mut rx2] {
            match rx.try_recv().expect("frame delivered") {
                Message::Text(s) => {
                    let v: Value = serde_json::from_str(&s).unwrap();
                    assert_eq!(v["t"], json!("event"));
                    assert_eq!(v["event"], json!("app-state"));
                    assert_eq!(v["payload"], json!({ "count": 3 }));
                }
                other => panic!("expected text, got {other:?}"),
            }
        }
    }

    #[test]
    fn fan_out_unknown_event_is_a_noop() {
        let inner: Arc<Mutex<HashMap<String, EventEntry>>> = Arc::new(Mutex::new(HashMap::new()));
        // Must not panic when no one is listening.
        fan_out(&inner, "nobody-home", "{}");
    }
}
