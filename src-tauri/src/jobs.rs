//! Scheduling helpers for the periodic background loops in `lib.rs`.
//!
//! The loops themselves stay in `lib.rs` because they need the `AppHandle`.
//! What lives here is the decision-making worth testing on its own: which
//! workspaces a given tick is allowed to touch, which of those actually need
//! git subprocesses run for them, and how long a loop waits before its first
//! tick so the timers don't phase-align.

use std::collections::HashSet;
use std::time::Duration;

use rand::Rng;

/// Non-active workspaces are refreshed on one tick in `GIT_SWEEP_STRIDE`.
/// At the 5 s tick that means every ~30 s for a background row, which is
/// well inside the window where a sidebar branch label reads as live, while
/// the per-tick subprocess count drops to roughly `N / 6 + 1`.
pub const GIT_SWEEP_STRIDE: u64 = 6;

/// One workspace's slot in a git-sweep tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSweepStep {
    pub workspace_id: String,
    pub cwd: String,
    /// False when an earlier step in the same tick already gathered this
    /// exact checkout — the caller reuses that result instead of forking
    /// the same git subprocesses a second time. Two workspaces pointed at
    /// one directory produce identical branch / ahead-behind / diff-stat
    /// output by construction.
    pub gather: bool,
}

/// Decide which workspaces a git-sweep tick refreshes.
///
/// The active workspace is refreshed on every tick — it is the row the user
/// is looking at and the one an agent is most likely changing under them.
/// Every other workspace is assigned to a stride bucket by list position, so
/// the background fleet is spread evenly across `stride` ticks instead of
/// landing on one expensive tick together.
///
/// `workspaces` must be in a stable order (sort by workspace id) or a
/// workspace can starve by shifting buckets every tick.
pub fn plan_git_sweep(
    tick: u64,
    workspaces: &[(String, String)],
    active_id: Option<&str>,
    stride: u64,
) -> Vec<GitSweepStep> {
    let stride = stride.max(1);
    let mut seen: HashSet<&str> = HashSet::new();
    let mut steps = Vec::new();
    for (index, (workspace_id, cwd)) in workspaces.iter().enumerate() {
        let is_active = active_id == Some(workspace_id.as_str());
        if !is_active && (index as u64) % stride != tick % stride {
            continue;
        }
        steps.push(GitSweepStep {
            workspace_id: workspace_id.clone(),
            cwd: cwd.clone(),
            gather: seen.insert(cwd.as_str()),
        });
    }
    steps
}

/// Collapse `(cwd, project_root)` pairs into one fetch per repository.
///
/// Linked worktrees share the object store and the remote refs of their
/// project root, so one `git fetch --prune` updates every workspace opened
/// on that repo. Workspaces whose project root wasn't resolved fall back to
/// their own cwd, which still drops exact duplicates.
pub fn dedupe_fetch_targets(entries: &[(String, Option<String>)]) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut targets = Vec::new();
    for (cwd, project_root) in entries {
        if cwd.is_empty() {
            continue;
        }
        let key = project_root.as_deref().unwrap_or(cwd.as_str());
        let key = key.trim_end_matches('/');
        if seen.insert(key.to_string()) {
            targets.push(cwd.clone());
        }
    }
    targets
}

/// A random start-up delay in `0..=max`.
///
/// Every loop here sleeps a whole period before its first tick, so without
/// this they all fire on the same instant at every common multiple of their
/// periods (the 3 s port scan, the 5 s git sweep and the 60 s PR poll meet
/// every minute). Offsetting the starts spreads that collision out for the
/// life of the process.
pub fn startup_jitter(max: Duration) -> Duration {
    let max_ms = max.as_millis() as u64;
    if max_ms == 0 {
        return Duration::ZERO;
    }
    Duration::from_millis(rand::thread_rng().gen_range(0..=max_ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(id, cwd)| ((*id).to_string(), (*cwd).to_string()))
            .collect()
    }

    fn ids(steps: &[GitSweepStep]) -> Vec<String> {
        steps.iter().map(|s| s.workspace_id.clone()).collect()
    }

    #[test]
    fn active_workspace_refreshes_on_every_tick() {
        let workspaces = ws(&[("a", "/a"), ("b", "/b"), ("c", "/c")]);
        for tick in 0..12 {
            let steps = plan_git_sweep(tick, &workspaces, Some("c"), GIT_SWEEP_STRIDE);
            assert!(
                ids(&steps).contains(&"c".to_string()),
                "active workspace missing from tick {tick}"
            );
        }
    }

    #[test]
    fn background_workspaces_spread_across_the_stride() {
        let workspaces = ws(&[
            ("a", "/a"),
            ("b", "/b"),
            ("c", "/c"),
            ("d", "/d"),
            ("e", "/e"),
            ("f", "/f"),
        ]);
        // One workspace per tick, and every workspace covered exactly once
        // over a full stride.
        let mut covered: Vec<String> = Vec::new();
        for tick in 0..GIT_SWEEP_STRIDE {
            let steps = plan_git_sweep(tick, &workspaces, None, GIT_SWEEP_STRIDE);
            assert_eq!(steps.len(), 1, "tick {tick} refreshed {} workspaces", steps.len());
            covered.extend(ids(&steps));
        }
        covered.sort();
        assert_eq!(covered, vec!["a", "b", "c", "d", "e", "f"]);
    }

    #[test]
    fn active_workspace_is_not_planned_twice_on_its_own_stride_tick() {
        let workspaces = ws(&[("a", "/a"), ("b", "/b")]);
        let steps = plan_git_sweep(0, &workspaces, Some("a"), 2);
        assert_eq!(ids(&steps), vec!["a"]);
    }

    #[test]
    fn identical_cwds_gather_once_per_tick() {
        let workspaces = ws(&[("a", "/repo"), ("b", "/repo"), ("c", "/other")]);
        let steps = plan_git_sweep(0, &workspaces, None, 1);
        assert_eq!(ids(&steps), vec!["a", "b", "c"]);
        assert_eq!(
            steps.iter().map(|s| s.gather).collect::<Vec<_>>(),
            vec![true, false, true],
            "the second workspace on /repo must reuse the first gather"
        );
    }

    #[test]
    fn stride_zero_is_treated_as_every_tick() {
        let workspaces = ws(&[("a", "/a"), ("b", "/b")]);
        assert_eq!(ids(&plan_git_sweep(3, &workspaces, None, 0)), vec!["a", "b"]);
    }

    #[test]
    fn fetch_targets_dedupe_by_project_root() {
        let entries = vec![
            ("/repo".to_string(), Some("/repo".to_string())),
            ("/repo/.worktrees/feature".to_string(), Some("/repo".to_string())),
            ("/other".to_string(), Some("/other/".to_string())),
            ("/other".to_string(), None),
            ("/loose".to_string(), None),
        ];
        assert_eq!(
            dedupe_fetch_targets(&entries),
            vec!["/repo".to_string(), "/other".to_string(), "/loose".to_string()],
        );
    }

    #[test]
    fn fetch_targets_skip_empty_cwds() {
        let entries = vec![(String::new(), None), ("/repo".to_string(), None)];
        assert_eq!(dedupe_fetch_targets(&entries), vec!["/repo".to_string()]);
    }

    #[test]
    fn startup_jitter_stays_within_bounds() {
        assert_eq!(startup_jitter(Duration::ZERO), Duration::ZERO);
        for _ in 0..64 {
            let delay = startup_jitter(Duration::from_secs(2));
            assert!(delay <= Duration::from_secs(2), "jitter overshot: {delay:?}");
        }
    }
}
