//! RFC 5545 recurrence handling for automations.
//!
//! An automation's `schedule` field is a complete iCalendar block — a
//! `DTSTART` line plus exactly one `RRULE` line, for example:
//!
//! ```text
//! DTSTART;TZID=America/New_York:20260101T090000
//! RRULE:FREQ=DAILY
//! ```
//!
//! This module wraps the `rrule` crate to validate such strings and to
//! compute the next fire time. The `rrule` crate evaluates occurrences
//! against the `DTSTART` timezone, so DST transitions are handled
//! correctly (a `BYHOUR=9` daily rule stays at 09:00 wall-clock across
//! a clock change).

use chrono::{DateTime, Utc};
use rrule::{RRuleSet, Tz};

/// Validate a schedule string, returning a human-readable error the
/// UI / MCP can surface verbatim.
pub fn validate(schedule: &str) -> Result<(), String> {
    parse(schedule).map(|_| ())
}

/// Compute the first occurrence strictly after `after`.
///
/// Returns `Ok(None)` when the rule has no further occurrences — a
/// bounded `COUNT=` / `UNTIL=` rule that is already exhausted. The
/// caller treats that as "disable this automation".
pub fn next_occurrence(
    schedule: &str,
    after: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, String> {
    let rule = parse(schedule)?;
    // The `rrule` crate's `after` bound is *inclusive*, so an occurrence
    // landing exactly on `after` would be returned and a just-fired
    // automation could immediately re-fire. We want the strictly-next
    // fire, so ask for a small batch and skip anything at or before
    // `after` (at most one occurrence can equal it).
    let result = rule.after(after.with_timezone(&Tz::UTC)).all(4);
    Ok(result
        .dates
        .into_iter()
        .map(|dt| dt.with_timezone(&Utc))
        .find(|dt| *dt > after))
}

/// Parse a schedule string into an `RRuleSet`, mapping every failure
/// mode to a flat `String` error.
fn parse(schedule: &str) -> Result<RRuleSet, String> {
    let trimmed = schedule.trim();
    if trimmed.is_empty() {
        return Err("Schedule is empty".to_string());
    }
    trimmed
        .parse::<RRuleSet>()
        .map_err(|e| format!("Invalid recurrence rule: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn validate_accepts_a_well_formed_daily_rule() {
        let schedule = "DTSTART:20260101T090000Z\nRRULE:FREQ=DAILY";
        assert!(validate(schedule).is_ok());
    }

    #[test]
    fn validate_rejects_an_empty_schedule() {
        assert!(validate("").is_err());
        assert!(validate("   \n  ").is_err());
    }

    #[test]
    fn validate_accepts_a_tzid_anchored_rule() {
        // The schedule builder in the UI emits TZID-anchored DTSTART
        // lines so daily/weekly rules keep their wall-clock time across
        // DST. Confirm the crate parses that form.
        let schedule = "DTSTART;TZID=America/New_York:20260101T090000\nRRULE:FREQ=DAILY";
        assert!(validate(schedule).is_ok());
        let next = next_occurrence(
            schedule,
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        )
        .unwrap();
        assert!(next.is_some());
    }

    #[test]
    fn next_occurrence_preserves_wall_clock_time_across_dst() {
        // A daily 06:00 America/Los_Angeles rule must keep firing at 06:00
        // local wall-clock time across the spring-forward (Sun 2026-03-08,
        // 02:00 PST → 03:00 PDT). The UTC instant therefore shifts by an hour:
        // 06:00 PST = 14:00 UTC before, 06:00 PDT = 13:00 UTC after. (Superset's
        // hand-rolled scheduler drifted here; codemux delegates to the rrule
        // crate + a TZID-anchored DTSTART, which the UI builder emits.)
        let schedule =
            "DTSTART;TZID=America/Los_Angeles:20260301T060000\nRRULE:FREQ=DAILY";
        let before_dst =
            next_occurrence(schedule, Utc.with_ymd_and_hms(2026, 3, 7, 0, 0, 0).unwrap())
                .unwrap()
                .unwrap();
        assert_eq!(before_dst, Utc.with_ymd_and_hms(2026, 3, 7, 14, 0, 0).unwrap());
        let after_dst =
            next_occurrence(schedule, Utc.with_ymd_and_hms(2026, 3, 9, 0, 0, 0).unwrap())
                .unwrap()
                .unwrap();
        assert_eq!(after_dst, Utc.with_ymd_and_hms(2026, 3, 9, 13, 0, 0).unwrap());
    }

    #[test]
    fn validate_rejects_a_rule_without_a_dtstart() {
        // `RRULE` alone is not a valid `RRuleSet` — it has no anchor.
        assert!(validate("RRULE:FREQ=DAILY").is_err());
    }

    #[test]
    fn validate_rejects_garbage() {
        assert!(validate("not a recurrence rule").is_err());
    }

    #[test]
    fn next_occurrence_finds_the_following_day() {
        let schedule = "DTSTART:20260101T090000Z\nRRULE:FREQ=DAILY";
        let after = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
        let next = next_occurrence(schedule, after).unwrap();
        assert_eq!(next, Some(Utc.with_ymd_and_hms(2026, 1, 2, 9, 0, 0).unwrap()));
    }

    #[test]
    fn next_occurrence_is_exclusive_of_the_after_instant() {
        // `after` lands exactly on a 09:00 occurrence — that instant
        // must be skipped and the following day returned.
        let schedule = "DTSTART:20260101T090000Z\nRRULE:FREQ=DAILY";
        let after = Utc.with_ymd_and_hms(2026, 1, 5, 9, 0, 0).unwrap();
        let next = next_occurrence(schedule, after).unwrap();
        assert_eq!(next, Some(Utc.with_ymd_and_hms(2026, 1, 6, 9, 0, 0).unwrap()));
    }

    #[test]
    fn next_occurrence_returns_none_when_a_bounded_rule_is_exhausted() {
        // Daily for three days starting 2026-01-01; asking after the
        // last occurrence yields nothing.
        let schedule = "DTSTART:20260101T090000Z\nRRULE:FREQ=DAILY;COUNT=3";
        let after = Utc.with_ymd_and_hms(2026, 1, 10, 0, 0, 0).unwrap();
        assert_eq!(next_occurrence(schedule, after).unwrap(), None);
    }

    #[test]
    fn next_occurrence_propagates_a_parse_error() {
        assert!(next_occurrence("garbage", Utc::now()).is_err());
    }
}
