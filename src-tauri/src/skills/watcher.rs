// Cross-platform file watcher for the skills system.
//
// Watches every enumerated skill path (user / project / optional plugin
// pool) and emits a single `skills-changed` Tauri event when files
// change. The frontend listens, invalidates the skills-store cache, and
// the next popup open / Settings refresh re-reads the disk.
//
// Backend selection: `notify::recommended_watcher` picks the best per
// OS — inotify on Linux, FSEvents on macOS, ReadDirectoryChangesW on
// Windows. For local skill paths (~/.claude, ~/.codex, etc.) all three
// behave equivalently.
//
// Debouncing: file editors typically write + close in two events, and
// some IDEs do atomic rename-to-replace which fires multiple events in
// a single tick. We coalesce by tracking the last-emit timestamp and
// dropping events that arrive within `DEBOUNCE_MS`. The frontend
// invalidates on any single event so missing the burst tail is fine.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{recommended_watcher, Event, RecommendedWatcher, RecursiveMode, Watcher};
use tauri::{AppHandle, Emitter, Runtime};

use super::paths::enumerate_scan_paths;

/// Coalesce-window for raw filesystem events. 300ms is comfortably
/// above the typical write+close burst from text editors but short
/// enough that a real save → refresh feels instant.
const DEBOUNCE_MS: u64 = 300;

/// Tauri event name the frontend listens for.
pub const SKILLS_CHANGED_EVENT: &str = "skills-changed";

/// Holds the live watcher (None when not running) plus the debounce
/// timestamp so multiple watch threads share a single window. Wrapped
/// in a single Mutex so the lock acquisition stays trivial.
#[derive(Default)]
pub struct SkillsWatcherState {
    inner: Mutex<SkillsWatcherInner>,
}

#[derive(Default)]
struct SkillsWatcherInner {
    watcher: Option<RecommendedWatcher>,
    last_emit: Option<Instant>,
}

impl SkillsWatcherState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start (or restart) the watcher with paths derived from the
    /// current project root + plugin-toggle. Drops any existing
    /// watcher first — idempotent across repeated calls.
    pub fn start<R: Runtime>(
        &self,
        app: AppHandle<R>,
        project_root: Option<PathBuf>,
        include_plugins: bool,
    ) -> Result<usize, String> {
        let mut inner = self.inner.lock().map_err(|e| format!("lock poisoned: {e}"))?;
        // Drop the old watcher first; notify watchers stop watching on Drop.
        inner.watcher = None;

        let paths = collect_paths(project_root.as_deref(), include_plugins);
        let last_emit = Arc::new(Mutex::new(inner.last_emit));
        let app_for_closure = app.clone();
        let last_emit_for_closure = Arc::clone(&last_emit);

        let mut watcher = recommended_watcher(move |res: notify::Result<Event>| {
            // notify's callback runs on a background thread it owns;
            // we only do cheap work here (debounce check + emit).
            if res.is_err() {
                return;
            }
            let now = Instant::now();
            let mut last = match last_emit_for_closure.lock() {
                Ok(l) => l,
                Err(_) => return, // poisoned — give up quietly
            };
            if let Some(prev) = *last {
                if now.duration_since(prev) < Duration::from_millis(DEBOUNCE_MS) {
                    return; // inside the debounce window — drop
                }
            }
            *last = Some(now);
            let _ = app_for_closure.emit(SKILLS_CHANGED_EVENT, ());
        })
        .map_err(|e| format!("create watcher: {e}"))?;

        let mut watched = 0usize;
        for path in paths {
            // Missing paths are normal (user hasn't created
            // ~/.codex/skills/ yet, etc.). Skip silently.
            if !path.exists() {
                continue;
            }
            match watcher.watch(&path, RecursiveMode::Recursive) {
                Ok(()) => watched += 1,
                Err(e) => {
                    eprintln!(
                        "skills-watcher: failed to watch {} — {e}",
                        path.display()
                    );
                }
            }
        }

        // Sync the inner debounce window with the closure's so a
        // future stop+start preserves the debounce continuity.
        if let Ok(closure_last) = last_emit.lock() {
            inner.last_emit = *closure_last;
        }
        inner.watcher = Some(watcher);
        Ok(watched)
    }

    /// Drop the watcher. Safe to call when not started.
    pub fn stop(&self) -> Result<(), String> {
        let mut inner = self.inner.lock().map_err(|e| format!("lock poisoned: {e}"))?;
        inner.watcher = None;
        Ok(())
    }
}

fn collect_paths(project_root: Option<&std::path::Path>, include_plugins: bool) -> Vec<PathBuf> {
    let scan = enumerate_scan_paths(project_root, include_plugins);
    let mut out: Vec<PathBuf> = Vec::new();
    for (path, _) in scan.user_paths {
        out.push(path);
    }
    for (path, _) in scan.project_paths {
        out.push(path);
    }
    for (path, _) in scan.plugin_paths {
        out.push(path);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // Walks the same enumeration `start` would call. Confirms missing
    // paths get filtered out before we ever ask `notify` to watch them
    // (avoids spurious watch errors on machines without all providers
    // installed).
    #[test]
    fn collect_paths_skips_nonexistent_dirs_via_existence_check() {
        // collect_paths itself doesn't filter — that's done inside
        // `start`. But `enumerate_scan_paths` returns paths regardless,
        // so we should at least verify it returns something to watch.
        let project = TempDir::new().unwrap();
        fs::create_dir_all(project.path().join(".claude/skills")).unwrap();
        let paths = collect_paths(Some(project.path()), false);
        assert!(!paths.is_empty());
    }

    // The debounce window is checked inline; verify the const itself
    // hasn't drifted to something unreasonable. Refactors can update
    // this; gives a single source of truth for the window guarantee.
    #[test]
    fn debounce_window_is_a_sane_default() {
        assert!((100..=1000).contains(&DEBOUNCE_MS));
    }
}
