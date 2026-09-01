//! Live per-host status as observed by the inventory poller.
//!
//! The Devices page shows one card per configured SSH host: online /
//! offline, when it was last seen, how much disk its workspaces use and
//! whether a Codemux Remote Control server is running there.
//!
//! Two kinds of fact feed a card:
//!
//! - **Persisted** — `last_seen_at`, `disk_bytes` — live as local-only
//!   columns on the `hosts` row (see `database.rs`) so a freshly launched
//!   app shows "last seen 2h ago" and a disk figure while the first poll
//!   is still warming up. They are never synced: they describe what THIS
//!   install saw over SSH.
//! - **Live** — `reachable`, `last_error`, `remote_control_serving` —
//!   only mean something for the current process and are held here in
//!   memory. A host with no live entry has not been probed yet.
//!
//! `hosts_inventory` feeds one [`Observation`] per host per tick into
//! [`HostStatusStore::apply`]; the pure [`next_live`] transition decides
//! how an observation changes the live bits, and is what unit tests pin
//! down. [`HostStatusStore::views_for`] overlays the live bits onto each
//! host record's persisted columns to build the wire shape.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::database::HostRecord;

/// Event emitted with a `Vec<HostStatusView>` payload whenever a poll
/// round changed any host's status. Mirrors the frontend constant.
pub const HOSTS_STATUS_CHANGED_EVENT: &str = "hosts-status-changed";

/// Wire shape for the frontend (`HostStatusView` in commands.ts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostStatusView {
    /// `HostView.id` — the local `hosts.id`.
    pub host_id: i64,
    /// This process has completed at least one live probe of the host.
    /// `false` means every other live field is a default, not a finding.
    pub probed: bool,
    /// The last live probe succeeded.
    pub reachable: bool,
    /// RFC 3339 timestamp of the last successful probe (persisted).
    pub last_seen_at: Option<String>,
    /// Why the host is unreachable, or — while reachable — why the last
    /// tick fell short (host agent missing, inventory failed/timed out).
    pub last_error: Option<String>,
    /// Sum of the host's workspace directories in bytes (persisted).
    /// `None` while unknown.
    pub disk_bytes: Option<u64>,
    /// A Codemux Remote Control server is up on the host.
    pub remote_control_serving: bool,
}

/// The process-local bits of a host's status. Presence of an entry in
/// [`HostStatusStore`] is what `probed` means.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveStatus {
    pub reachable: bool,
    pub last_error: Option<String>,
    pub remote_control_serving: bool,
}

/// The optional facts the `workspace list` envelope carries. A daemon
/// that predates them omits both.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HostFacts {
    /// `None` when the host skipped the walk (`CODEMUX_SKIP_DISK=1`),
    /// ran out of budget, or is too old to report it.
    pub disk_bytes: Option<u64>,
    /// `None` when the daemon is too old to report it.
    pub remote_control_serving: Option<bool>,
}

/// What one poll tick learned about a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observation {
    /// SSH itself failed (offline, DNS, refused, key not authorized,
    /// timed out).
    Unreachable { reason: String },
    /// SSH connected, but the tick could not complete — the binary is
    /// missing, or the inventory fetch/parse failed or timed out. The
    /// host counts as seen; the error is surfaced so the card can
    /// explain itself.
    Degraded { reason: String },
    /// SSH connected and the inventory came back.
    Reachable { facts: HostFacts },
}

/// Pure transition: fold one observation into the previous live bits.
pub fn next_live(prev: &LiveStatus, observation: &Observation) -> LiveStatus {
    match observation {
        Observation::Unreachable { reason } => LiveStatus {
            reachable: false,
            last_error: Some(reason.clone()),
            // A server we can't reach is not one the user can open.
            remote_control_serving: false,
        },
        Observation::Degraded { reason } => LiveStatus {
            reachable: true,
            last_error: Some(reason.clone()),
            // The envelope was unreadable, so we learned nothing about
            // the server either way; keep the last answer rather than
            // flashing the "open" affordance off and on.
            remote_control_serving: prev.remote_control_serving,
        },
        Observation::Reachable { facts } => LiveStatus {
            reachable: true,
            last_error: None,
            // An old daemon that doesn't report the flag can't be
            // offering the feature either.
            remote_control_serving: facts.remote_control_serving.unwrap_or(false),
        },
    }
}

/// In-memory live-status map, managed as Tauri state. Updated by the
/// poller and read by the `hosts_status_list` command.
#[derive(Default)]
pub struct HostStatusStore {
    inner: Mutex<HashMap<i64, LiveStatus>>,
}

impl HostStatusStore {
    /// Fold an observation in. Returns `true` when the live bits changed
    /// (including the first observation for a host), so the poller can
    /// decide whether to emit an event. A repeat of the same outcome —
    /// the common steady state — reports no change even though the
    /// persisted `last_seen_at` advanced.
    pub fn apply(&self, host_id: i64, observation: &Observation) -> bool {
        let mut map = self.inner.lock().unwrap();
        let next = next_live(&map.get(&host_id).cloned().unwrap_or_default(), observation);
        match map.get(&host_id) {
            Some(prev) if *prev == next => false,
            _ => {
                map.insert(host_id, next);
                true
            }
        }
    }

    pub fn get(&self, host_id: i64) -> Option<LiveStatus> {
        self.inner.lock().unwrap().get(&host_id).cloned()
    }

    pub fn remove(&self, host_id: i64) {
        self.inner.lock().unwrap().remove(&host_id);
    }

    /// One row per configured host, in the order given: the record's
    /// persisted facts overlaid with whatever this process has observed.
    /// Hosts that were never probed come back `probed: false`.
    pub fn views_for(&self, hosts: &[HostRecord]) -> Vec<HostStatusView> {
        let map = self.inner.lock().unwrap();
        hosts
            .iter()
            .map(|h| {
                let live = map.get(&h.id);
                let bits = live.cloned().unwrap_or_default();
                HostStatusView {
                    host_id: h.id,
                    probed: live.is_some(),
                    reachable: bits.reachable,
                    last_seen_at: h.last_seen_at.clone(),
                    last_error: bits.last_error,
                    disk_bytes: h.disk_bytes,
                    remote_control_serving: bits.remote_control_serving,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn online(serving: bool) -> LiveStatus {
        LiveStatus {
            reachable: true,
            last_error: None,
            remote_control_serving: serving,
        }
    }

    fn reachable(disk: Option<u64>, serving: Option<bool>) -> Observation {
        Observation::Reachable {
            facts: HostFacts {
                disk_bytes: disk,
                remote_control_serving: serving,
            },
        }
    }

    // ── transitions ────────────────────────────────────────────

    #[test]
    fn unreachable_records_reason_and_clears_serving() {
        let next = next_live(
            &online(true),
            &Observation::Unreachable {
                reason: "ssh: connect timed out".into(),
            },
        );
        assert!(!next.reachable);
        assert_eq!(next.last_error.as_deref(), Some("ssh: connect timed out"));
        assert!(!next.remote_control_serving);
    }

    #[test]
    fn degraded_is_reachable_with_error_and_keeps_serving() {
        let next = next_live(
            &online(true),
            &Observation::Degraded {
                reason: "codemux-remote missing".into(),
            },
        );
        assert!(next.reachable);
        assert_eq!(next.last_error.as_deref(), Some("codemux-remote missing"));
        assert!(
            next.remote_control_serving,
            "an unreadable envelope says nothing about the server; keep the last answer"
        );

        let from_off = next_live(
            &online(false),
            &Observation::Degraded {
                reason: "inventory timed out after 20s".into(),
            },
        );
        assert!(!from_off.remote_control_serving);
    }

    #[test]
    fn reachable_clears_error_and_reads_serving_from_facts() {
        let prev = LiveStatus {
            reachable: false,
            last_error: Some("was down".into()),
            remote_control_serving: false,
        };
        let next = next_live(&prev, &reachable(Some(123), Some(true)));
        assert!(next.reachable);
        assert!(next.last_error.is_none());
        assert!(next.remote_control_serving);
    }

    #[test]
    fn old_daemon_without_facts_is_reachable_but_not_serving() {
        let next = next_live(&online(true), &reachable(None, None));
        assert!(next.reachable);
        assert!(!next.remote_control_serving);
    }

    // ── store ──────────────────────────────────────────────────

    #[test]
    fn apply_reports_change_on_transitions_only() {
        let store = HostStatusStore::default();
        assert!(store.apply(7, &reachable(Some(1), Some(false))), "first observation");
        assert!(
            !store.apply(7, &reachable(Some(1), Some(false))),
            "same outcome on a later tick is not a change (last_seen_at alone advancing \
             must not fan out an event)"
        );
        assert!(
            !store.apply(7, &reachable(Some(2), Some(false))),
            "disk figures live on the host record, not here"
        );
        assert!(store.apply(7, &reachable(Some(1), Some(true))), "serving flipped");
        assert!(store.apply(
            7,
            &Observation::Unreachable {
                reason: "down".into()
            },
        ));
        assert!(!store.apply(
            7,
            &Observation::Unreachable {
                reason: "down".into()
            },
        ));
        assert!(
            store.apply(
                7,
                &Observation::Unreachable {
                    reason: "different reason".into()
                },
            ),
            "a new reason is worth showing"
        );
        assert!(!store.get(7).unwrap().reachable);
    }

    #[test]
    fn views_overlay_live_bits_onto_persisted_columns() {
        let store = HostStatusStore::default();
        store.apply(1, &reachable(Some(5), Some(true)));
        let mut seen = host(1);
        seen.last_seen_at = Some("2026-08-27T10:00:00Z".into());
        seen.disk_bytes = Some(4096);
        let mut restored = host(2);
        restored.last_seen_at = Some("2026-08-26T10:00:00Z".into());
        restored.disk_bytes = Some(77);

        let views = store.views_for(&[seen, restored, host(3)]);
        assert_eq!(views.len(), 3);

        assert_eq!(views[0].host_id, 1);
        assert!(views[0].probed);
        assert!(views[0].reachable);
        assert!(views[0].remote_control_serving);
        assert_eq!(views[0].last_seen_at.as_deref(), Some("2026-08-27T10:00:00Z"));
        assert_eq!(views[0].disk_bytes, Some(4096), "disk comes from the record");

        // Persisted facts survive a restart; the live bits are defaults
        // until the first probe, flagged by `probed: false`.
        assert_eq!(views[1].host_id, 2);
        assert!(!views[1].probed);
        assert!(!views[1].reachable);
        assert!(views[1].last_error.is_none());
        assert_eq!(views[1].last_seen_at.as_deref(), Some("2026-08-26T10:00:00Z"));
        assert_eq!(views[1].disk_bytes, Some(77));

        assert_eq!(views[2].host_id, 3);
        assert!(!views[2].probed);
        assert!(views[2].last_seen_at.is_none());
        assert!(views[2].disk_bytes.is_none());
        assert!(!views[2].remote_control_serving);
    }

    #[test]
    fn remove_drops_the_live_entry() {
        let store = HostStatusStore::default();
        store.apply(4, &reachable(None, None));
        store.remove(4);
        assert!(store.get(4).is_none());
        assert!(!store.views_for(&[host(4)])[0].probed);
    }

    #[test]
    fn view_serializes_to_the_wire_contract() {
        let view = HostStatusView {
            host_id: 1,
            probed: true,
            reachable: true,
            last_seen_at: Some("t".into()),
            last_error: None,
            disk_bytes: Some(10),
            remote_control_serving: false,
        };
        let json = serde_json::to_string(&view).unwrap();
        assert_eq!(
            json,
            r#"{"host_id":1,"probed":true,"reachable":true,"last_seen_at":"t","last_error":null,"disk_bytes":10,"remote_control_serving":false}"#
        );
    }

    fn host(id: i64) -> HostRecord {
        HostRecord {
            id,
            server_id: None,
            name: format!("host-{id}"),
            ssh_target: format!("user@host-{id}"),
            created_at: String::new(),
            updated_at: String::new(),
            deleted_at: None,
            dirty: false,
            last_seen_at: None,
            disk_bytes: None,
            disk_measured_at: None,
        }
    }
}
