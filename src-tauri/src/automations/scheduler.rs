//! Scheduler decision logic for automations.
//!
//! This module is the pure core of the host-side scheduler: given an
//! automation and the current time it answers "should this fire now?"
//! and "what is the dedup key for this fire?". The I/O loop that polls
//! the registry, creates a workspace and spawns the agent lives in the
//! `codemux-remote` binary and calls into here — keeping the decision
//! logic pure makes it unit-testable without a host or a clock.

use crate::automations::recurrence;
use crate::database::{AutomationRecord, AutomationRunRecord, DatabaseStore};
use chrono::{DateTime, SecondsFormat, Timelike, Utc};

/// How often the host-side loop evaluates the schedule. A minute-grained
/// recurrence never needs finer resolution, and `fire_key` collapses any
/// jitter within a minute to a single idempotent run.
pub const TICK_INTERVAL_SECS: u64 = 60;

/// Floor an instant to the start of its minute and format it as the
/// canonical `scheduled_for` dedup key — RFC 3339, UTC, second
/// precision. `record_automation_run`'s `UNIQUE(automation_id,
/// scheduled_for)` constraint then makes a re-delivered tick within the
/// same minute a no-op insert.
pub fn fire_key(at: DateTime<Utc>) -> String {
    at.with_second(0)
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(at)
        .to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Whether an automation is due to fire at `now`: it must be enabled and
/// carry a `next_run_at` that has arrived. A paused automation, or one
/// whose recurrence is exhausted (no `next_run_at`), is never due.
pub fn is_due(automation: &AutomationRecord, now: DateTime<Utc>) -> bool {
    if !automation.enabled {
        return false;
    }
    match automation.next_run_at.as_deref().and_then(parse_utc) {
        Some(next) => next <= now,
        None => false,
    }
}

/// Parse an RFC 3339 timestamp to `DateTime<Utc>`, returning `None` on
/// any malformed value rather than panicking the scheduler loop.
fn parse_utc(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Whether an automation already has a run that has not reached a
/// terminal state — used for the one-run-at-a-time overlap guard.
fn has_active_run(db: &DatabaseStore, automation_id: i64) -> bool {
    db.list_automation_runs(automation_id, 20)
        .iter()
        .any(|run| run.status == "scheduled" || run.status == "running")
}

/// Evaluate the whole schedule once.
///
/// For every due automation this:
/// 1. records a run for the current minute — `scheduled` normally, or
///    `skipped_busy` when a previous run is still in flight (overlap is
///    serialised per automation, not stacked);
/// 2. advances `next_run_at` to the following occurrence, or clears it
///    when a bounded recurrence is exhausted.
///
/// The per-minute `fire_key` plus the `UNIQUE(automation_id,
/// scheduled_for)` constraint make a tick idempotent, so a missed or
/// double tick never produces duplicate runs.
///
/// Returns the freshly-created `scheduled` runs — the caller (the
/// executor) turns each into a workspace + agent. `skipped_busy` runs
/// are recorded but not returned, as there is nothing to execute.
///
/// `local_only` is the host-routing switch: the desktop scheduler
/// passes `true` and runs only automations targeting this machine
/// (`host_id IS NULL`) — a host-assigned automation is the job of that
/// host's `codemux-remote scheduler`, which passes `false`.
pub fn tick(
    db: &DatabaseStore,
    now: DateTime<Utc>,
    local_only: bool,
) -> Vec<AutomationRunRecord> {
    let mut fired = Vec::new();

    for automation in db.list_automations() {
        if local_only && automation.host_id.is_some() {
            continue;
        }
        if !is_due(&automation, now) {
            continue;
        }

        let key = fire_key(now);
        let busy = has_active_run(db, automation.id);
        let status = if busy { "skipped_busy" } else { "scheduled" };

        match db.record_automation_run(automation.id, status, &key, automation.host_id, None) {
            Ok(Some(run)) => {
                if busy {
                    eprintln!(
                        "[codemux::scheduler] automation {} skipped: previous run still active",
                        automation.id
                    );
                } else {
                    fired.push(run);
                }
            }
            // `None` means this minute was already recorded — idempotent.
            Ok(None) => {}
            Err(error) => {
                eprintln!(
                    "[codemux::scheduler] failed to record run for automation {}: {error}",
                    automation.id
                );
            }
        }

        // Advance the schedule regardless of the run outcome, so the
        // automation moves on to its next slot instead of re-firing
        // this one.
        match recurrence::next_occurrence(&automation.schedule, now) {
            Ok(next) => {
                let next_str = next.map(|dt| dt.to_rfc3339());
                if let Err(error) =
                    db.set_automation_next_run(automation.id, next_str.as_deref())
                {
                    eprintln!(
                        "[codemux::scheduler] failed to advance automation {}: {error}",
                        automation.id
                    );
                }
            }
            Err(error) => {
                // A schedule we can't parse must not leave `next_run_at`
                // in the past — that would re-fire the automation on
                // every tick. Clear it so the automation goes dormant
                // instead of spamming runs once a minute.
                eprintln!(
                    "[codemux::scheduler] invalid schedule on automation {}: {error} \
                     — clearing next run so it stops re-firing",
                    automation.id
                );
                if let Err(clear_error) =
                    db.set_automation_next_run(automation.id, None)
                {
                    eprintln!(
                        "[codemux::scheduler] failed to clear next run for automation {}: {clear_error}",
                        automation.id
                    );
                }
            }
        }
    }

    fired
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// Build an `AutomationRecord` with the given enabled flag and
    /// `next_run_at`; other fields are irrelevant to scheduling.
    fn automation(enabled: bool, next_run_at: Option<&str>) -> AutomationRecord {
        AutomationRecord {
            id: 1,
            server_id: None,
            name: "Test".to_string(),
            prompt: "do the thing".to_string(),
            agent: "claude".to_string(),
            schedule: "DTSTART:20260101T090000Z\nRRULE:FREQ=DAILY".to_string(),
            timezone: "UTC".to_string(),
            host_id: None,
            project_path: None,
            project_remote: None,
            enabled,
            retention_limit: 10,
            last_run_at: None,
            next_run_at: next_run_at.map(str::to_string),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            deleted_at: None,
            dirty: false,
        }
    }

    #[test]
    fn fire_key_floors_to_the_minute() {
        let at = Utc.with_ymd_and_hms(2026, 1, 2, 9, 7, 43).unwrap();
        assert_eq!(fire_key(at), "2026-01-02T09:07:00Z");
    }

    #[test]
    fn fire_key_is_stable_across_a_minute() {
        let early = Utc.with_ymd_and_hms(2026, 1, 2, 9, 7, 1).unwrap();
        let late = Utc.with_ymd_and_hms(2026, 1, 2, 9, 7, 59).unwrap();
        assert_eq!(fire_key(early), fire_key(late));
    }

    #[test]
    fn due_when_enabled_and_next_run_has_arrived() {
        let now = Utc.with_ymd_and_hms(2026, 1, 2, 9, 0, 0).unwrap();
        let a = automation(true, Some("2026-01-02T09:00:00+00:00"));
        assert!(is_due(&a, now));

        let earlier = automation(true, Some("2026-01-02T08:00:00+00:00"));
        assert!(is_due(&earlier, now));
    }

    #[test]
    fn not_due_when_next_run_is_in_the_future() {
        let now = Utc.with_ymd_and_hms(2026, 1, 2, 9, 0, 0).unwrap();
        let a = automation(true, Some("2026-01-02T10:00:00+00:00"));
        assert!(!is_due(&a, now));
    }

    #[test]
    fn not_due_when_paused_even_if_next_run_has_arrived() {
        let now = Utc.with_ymd_and_hms(2026, 1, 2, 9, 0, 0).unwrap();
        let a = automation(false, Some("2026-01-02T09:00:00+00:00"));
        assert!(!is_due(&a, now));
    }

    #[test]
    fn not_due_when_recurrence_is_exhausted() {
        let now = Utc.with_ymd_and_hms(2026, 1, 2, 9, 0, 0).unwrap();
        let a = automation(true, None);
        assert!(!is_due(&a, now));
    }

    #[test]
    fn not_due_when_next_run_is_malformed() {
        // A garbage timestamp must not panic the loop — it is simply
        // never due until the value is corrected.
        let now = Utc::now();
        let a = automation(true, Some("not-a-timestamp"));
        assert!(!is_due(&a, now));
    }

    // ── tick ──

    /// Insert a daily automation and set its `next_run_at`, returning the
    /// stored record.
    fn insert_due(db: &DatabaseStore, next_run: Option<&str>) -> AutomationRecord {
        let a = db
            .insert_automation(&crate::database::AutomationInput {
                name: "Test".to_string(),
                prompt: "do the thing".to_string(),
                agent: "claude".to_string(),
                schedule: "DTSTART:20260101T090000Z\nRRULE:FREQ=DAILY".to_string(),
                timezone: "UTC".to_string(),
                host_id: None,
                project_path: None,
                project_remote: None,
                retention_limit: 10,
            })
            .unwrap();
        db.set_automation_next_run(a.id, next_run).unwrap();
        db.get_automation(a.id).unwrap()
    }

    #[test]
    fn tick_fires_a_due_automation_and_advances_it() {
        let db = crate::database::init_test_database();
        let now = Utc.with_ymd_and_hms(2026, 1, 2, 9, 0, 0).unwrap();
        let a = insert_due(&db, Some("2026-01-02T09:00:00+00:00"));

        let fired = tick(&db, now, false);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].automation_id, a.id);
        assert_eq!(fired[0].status, "scheduled");

        // The schedule moved on, so the same slot will not fire again.
        let advanced = db.get_automation(a.id).unwrap();
        assert!(advanced.next_run_at.is_some());
        assert_ne!(
            advanced.next_run_at.as_deref(),
            Some("2026-01-02T09:00:00+00:00")
        );
    }

    #[test]
    fn tick_ignores_an_automation_whose_next_run_is_in_the_future() {
        let db = crate::database::init_test_database();
        let now = Utc.with_ymd_and_hms(2026, 1, 2, 9, 0, 0).unwrap();
        insert_due(&db, Some("2026-01-02T10:00:00+00:00"));

        assert!(tick(&db, now, false).is_empty());
    }

    #[test]
    fn tick_does_not_refire_the_same_slot() {
        let db = crate::database::init_test_database();
        let now = Utc.with_ymd_and_hms(2026, 1, 2, 9, 0, 0).unwrap();
        let a = insert_due(&db, Some("2026-01-02T09:00:00+00:00"));

        assert_eq!(tick(&db, now, false).len(), 1);
        // A second tick at the same instant: the schedule has advanced,
        // so nothing fires and no duplicate run is recorded.
        assert!(tick(&db, now, false).is_empty());
        assert_eq!(db.list_automation_runs(a.id, 10).len(), 1);
    }

    #[test]
    fn tick_clears_next_run_on_an_unparseable_schedule() {
        // A schedule the recurrence engine can't parse must not pin the
        // automation at a past `next_run_at` — that would re-fire it on
        // every tick, once a minute, forever. `tick` clears
        // `next_run_at` instead, so the automation fires at most once
        // and then goes dormant.
        let db = crate::database::init_test_database();
        let now = Utc.with_ymd_and_hms(2026, 1, 2, 9, 0, 0).unwrap();
        let a = db
            .insert_automation(&crate::database::AutomationInput {
                name: "Broken".to_string(),
                prompt: "p".to_string(),
                agent: "claude".to_string(),
                schedule: "this is not a valid RRULE".to_string(),
                timezone: "UTC".to_string(),
                host_id: None,
                project_path: None,
                project_remote: None,
                retention_limit: 10,
            })
            .unwrap();
        db.set_automation_next_run(a.id, Some("2026-01-02T09:00:00+00:00"))
            .unwrap();

        // First tick fires the due slot, then fails to compute the next.
        assert_eq!(tick(&db, now, false).len(), 1);
        // `next_run_at` is cleared — the automation is now dormant.
        assert!(db.get_automation(a.id).unwrap().next_run_at.is_none());

        // A later tick must not fire it again — no runaway runs.
        let later = Utc.with_ymd_and_hms(2026, 1, 2, 9, 1, 0).unwrap();
        assert!(tick(&db, later, false).is_empty());
        assert_eq!(db.list_automation_runs(a.id, 10).len(), 1);
    }

    #[test]
    fn tick_records_skipped_busy_when_a_previous_run_is_still_active() {
        let db = crate::database::init_test_database();
        let now = Utc.with_ymd_and_hms(2026, 1, 2, 9, 0, 0).unwrap();
        let a = insert_due(&db, Some("2026-01-02T09:00:00+00:00"));

        // A still-running run from an earlier fire.
        db.record_automation_run(a.id, "running", "2026-01-01T09:00:00Z", None, None)
            .unwrap();

        let fired = tick(&db, now, false);
        // Overlap: nothing is handed to the executor...
        assert!(fired.is_empty());
        // ...but the skip is recorded for this minute.
        let runs = db.list_automation_runs(a.id, 10);
        assert!(runs.iter().any(|r| r.status == "skipped_busy"));
    }

    #[test]
    fn tick_skips_a_paused_automation() {
        let db = crate::database::init_test_database();
        let now = Utc.with_ymd_and_hms(2026, 1, 2, 9, 0, 0).unwrap();
        let a = insert_due(&db, Some("2026-01-02T09:00:00+00:00"));
        db.set_automation_enabled(a.id, false).unwrap();

        assert!(tick(&db, now, false).is_empty());
        assert_eq!(db.list_automation_runs(a.id, 10).len(), 0);
    }

    #[test]
    fn tick_local_only_skips_host_assigned_automations() {
        let db = crate::database::init_test_database();
        let now = Utc.with_ymd_and_hms(2026, 1, 2, 9, 0, 0).unwrap();
        // An automation targeting a remote host (host_id = Some).
        let a = db
            .insert_automation(&crate::database::AutomationInput {
                name: "Remote".to_string(),
                prompt: "p".to_string(),
                agent: "claude".to_string(),
                schedule: "DTSTART:20260101T090000Z\nRRULE:FREQ=DAILY".to_string(),
                timezone: "UTC".to_string(),
                host_id: Some(7),
                project_path: None,
                project_remote: None,
                retention_limit: 10,
            })
            .unwrap();
        db.set_automation_next_run(a.id, Some("2026-01-02T09:00:00+00:00"))
            .unwrap();

        // Desktop scheduler (`local_only = true`): host-assigned
        // automation is skipped.
        assert!(tick(&db, now, true).is_empty());
        // Remote scheduler (`local_only = false`): it fires.
        assert_eq!(tick(&db, now, false).len(), 1);
    }
}
