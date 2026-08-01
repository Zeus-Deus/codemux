//! Shared per-session bookkeeping behind
//! [`ProviderRuntimeEvent::ContextUsageUpdated`].
//!
//! Every provider reports token usage in its own shape, but the rules
//! the canonical [`ContextUsageSnapshot`] must obey are identical
//! across adapters:
//!
//! * **Clamp** — `used_tokens` never exceeds a known `max_tokens`, so
//!   the meter can never render >100%.
//! * **Sticky window** — a provider reports its context-window size
//!   only on some messages (Claude on `result`, Codex on
//!   `thread/tokenUsage/updated`). Once observed it is remembered and
//!   stamped on every later snapshot. Never guessed.
//! * **Monotonic lifetime total** — `total_processed_tokens` only ever
//!   climbs, and is omitted unless it is strictly greater than
//!   `used_tokens` (a fresh thread would otherwise render a redundant
//!   duplicate line).
//! * **No-op suppression** — an identical repeat snapshot, or a
//!   computed occupancy of zero, produces no event at all.
//!
//! Adapters own one tracker per session and feed it their decoded
//! numbers; the tracker decides whether an event is warranted.

use super::events::{ContextUsageSnapshot, ProviderRuntimeEvent};
use super::types::ThreadId;

/// Per-session context-window accounting for one thread.
///
/// Cheap and `Default`-constructible so adapters can embed it in the
/// per-session translator state they already thread by `&mut`.
#[derive(Debug, Default, Clone)]
pub struct ContextUsageTracker {
    /// Latest provider-reported context-window size. Sticky: once a
    /// provider tells us the window, later snapshots keep carrying it
    /// even when the reporting message doesn't repeat it. `None` until
    /// a provider says a number — never inferred from a model name.
    max_tokens: Option<u64>,
    /// Monotonic lifetime token total for the thread. Either summed by
    /// the adapter (Claude: per-turn processed figures) or adopted from
    /// a provider that maintains its own lifetime counter (Codex).
    lifetime_total: u64,
    /// Last snapshot actually emitted, for no-op suppression.
    last_emitted: Option<ContextUsageSnapshot>,
    /// Most recent *unclamped* live occupancy fed to [`Self::snapshot`].
    /// Lets an adapter re-emit at a later checkpoint (Claude learns the
    /// window size only at turn end) without re-deriving the reading.
    last_live_used: u64,
}

impl ContextUsageTracker {
    /// Record a provider-reported context-window size. Zero and `None`
    /// are ignored so a provider that reports `0`/omits the field never
    /// clobbers a previously-known good window.
    pub fn observe_max_tokens(&mut self, max_tokens: Option<u64>) {
        if let Some(max) = max_tokens.filter(|m| *m > 0) {
            self.max_tokens = Some(max);
        }
    }

    /// The sticky context-window size, when a provider has reported one.
    pub fn max_tokens(&self) -> Option<u64> {
        self.max_tokens
    }

    /// Add a per-turn processed figure to the lifetime accumulator.
    ///
    /// For providers that do not maintain their own lifetime counter.
    /// Call exactly once per turn (at turn terminus) — calling it per
    /// assistant message would double-count cache reads, which repeat
    /// in full on every iteration of a turn.
    pub fn add_processed(&mut self, tokens: u64) {
        self.lifetime_total = self.lifetime_total.saturating_add(tokens);
    }

    /// Adopt a provider-maintained lifetime total. Clamped upward only,
    /// so a provider that resets or under-reports mid-session cannot
    /// make the lifetime figure regress.
    pub fn observe_lifetime_total(&mut self, total: u64) {
        self.lifetime_total = self.lifetime_total.max(total);
    }

    /// The lifetime total accumulated so far.
    pub fn lifetime_total(&self) -> u64 {
        self.lifetime_total
    }

    /// The most recent live occupancy reading, unclamped. `0` when no
    /// reading has been taken yet.
    pub fn last_live_used(&self) -> u64 {
        self.last_live_used
    }

    /// Build the next snapshot, applying clamp / omit / suppression.
    ///
    /// Returns `None` when nothing should be emitted: a zero occupancy
    /// (no usable reading yet) or a snapshot identical to the last one
    /// emitted for this thread.
    ///
    /// `last_used_tokens` is passed through unclamped — it is a
    /// pre-compaction high-water mark, not a live occupancy, so
    /// squashing it to the window would erase the very fact it exists
    /// to record.
    pub fn snapshot(
        &mut self,
        used_tokens: u64,
        last_used_tokens: Option<u64>,
        compacts_automatically: Option<bool>,
    ) -> Option<ContextUsageSnapshot> {
        if used_tokens == 0 {
            return None;
        }
        self.last_live_used = used_tokens;
        let used = match self.max_tokens {
            Some(max) => used_tokens.min(max),
            None => used_tokens,
        };
        // The lifetime figure keeps the *real* number even when the
        // live occupancy was clamped down to the window.
        let total_processed_tokens = Some(self.lifetime_total).filter(|total| *total > used);
        let snapshot = ContextUsageSnapshot {
            used_tokens: used,
            total_processed_tokens,
            max_tokens: self.max_tokens,
            last_used_tokens: last_used_tokens.filter(|v| *v > 0),
            compacts_automatically,
        };
        if self.last_emitted.as_ref() == Some(&snapshot) {
            return None;
        }
        self.last_emitted = Some(snapshot.clone());
        Some(snapshot)
    }

    /// [`Self::snapshot`] wrapped as ready-to-broadcast events —
    /// empty when nothing should be emitted, so adapters can splice the
    /// result straight into their event vector.
    pub fn events(
        &mut self,
        thread_id: &ThreadId,
        used_tokens: u64,
        last_used_tokens: Option<u64>,
        compacts_automatically: Option<bool>,
    ) -> Vec<ProviderRuntimeEvent> {
        match self.snapshot(used_tokens, last_used_tokens, compacts_automatically) {
            Some(usage) => vec![ProviderRuntimeEvent::ContextUsageUpdated {
                thread_id: thread_id.clone(),
                usage,
            }],
            None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_occupancy_emits_nothing() {
        let mut t = ContextUsageTracker::default();
        assert!(t.snapshot(0, None, Some(true)).is_none());
    }

    #[test]
    fn identical_repeat_snapshot_is_suppressed() {
        let mut t = ContextUsageTracker::default();
        assert!(t.snapshot(100, None, Some(true)).is_some());
        assert!(t.snapshot(100, None, Some(true)).is_none());
        // A changed reading emits again.
        assert!(t.snapshot(101, None, Some(true)).is_some());
    }

    #[test]
    fn used_is_clamped_to_the_reported_window() {
        let mut t = ContextUsageTracker::default();
        t.observe_max_tokens(Some(200_000));
        let snap = t.snapshot(250_000, None, None).unwrap();
        assert_eq!(snap.used_tokens, 200_000);
        assert_eq!(snap.max_tokens, Some(200_000));
    }

    #[test]
    fn window_is_sticky_and_never_clobbered_by_zero_or_none() {
        let mut t = ContextUsageTracker::default();
        t.observe_max_tokens(Some(200_000));
        t.observe_max_tokens(None);
        t.observe_max_tokens(Some(0));
        assert_eq!(t.max_tokens(), Some(200_000));
    }

    #[test]
    fn total_processed_omitted_unless_strictly_greater_than_used() {
        let mut t = ContextUsageTracker::default();
        // Nothing accumulated yet → omitted.
        assert!(t.snapshot(500, None, None).unwrap().total_processed_tokens.is_none());
        // Equal to used → still omitted (redundant duplicate line).
        t.add_processed(600);
        assert!(t.snapshot(600, None, None).unwrap().total_processed_tokens.is_none());
        // Strictly greater → surfaced.
        t.add_processed(400);
        let snap = t.snapshot(700, None, None).unwrap();
        assert_eq!(snap.total_processed_tokens, Some(1000));
    }

    #[test]
    fn clamped_used_still_reports_the_real_lifetime_figure() {
        let mut t = ContextUsageTracker::default();
        t.observe_max_tokens(Some(200_000));
        t.add_processed(250_000);
        let snap = t.snapshot(250_000, None, None).unwrap();
        assert_eq!(snap.used_tokens, 200_000);
        assert_eq!(snap.total_processed_tokens, Some(250_000));
    }

    #[test]
    fn lifetime_total_never_regresses() {
        let mut t = ContextUsageTracker::default();
        t.observe_lifetime_total(5_000);
        t.observe_lifetime_total(1_000);
        assert_eq!(t.lifetime_total(), 5_000);
    }

    #[test]
    fn last_used_is_passed_through_unclamped_and_zero_is_dropped() {
        let mut t = ContextUsageTracker::default();
        t.observe_max_tokens(Some(200_000));
        let snap = t.snapshot(20_000, Some(410_000), None).unwrap();
        assert_eq!(snap.last_used_tokens, Some(410_000));
        let snap = t.snapshot(21_000, Some(0), None).unwrap();
        assert!(snap.last_used_tokens.is_none());
    }

    #[test]
    fn events_wraps_snapshot_into_a_thread_scoped_event() {
        let mut t = ContextUsageTracker::default();
        let tid = ThreadId("t1".into());
        let events = t.events(&tid, 42, None, Some(true));
        match events.as_slice() {
            [ProviderRuntimeEvent::ContextUsageUpdated { thread_id, usage }] => {
                assert_eq!(thread_id.0, "t1");
                assert_eq!(usage.used_tokens, 42);
                assert_eq!(usage.compacts_automatically, Some(true));
            }
            other => panic!("expected one ContextUsageUpdated, got {other:?}"),
        }
        assert!(t.events(&tid, 42, None, Some(true)).is_empty());
    }
}
