//! Terminal scrollback persistence.
//!
//! Saves and restores serialized xterm.js terminal buffers so that terminal
//! panes can show previous session content after an app restart.
//!
//! Storage layout:
//!   ~/.local/share/codemux/scrollback/{workspace_id}/{pane_id}.dat   — serialized terminal buffer
//!   ~/.local/share/codemux/scrollback/{workspace_id}/{pane_id}.json  — pane metadata sidecar

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// ── Types ──────────────────────────────────────────────────────

/// Metadata stored alongside each scrollback file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScrollbackMeta {
    pub pane_id: String,
    pub session_id: String,
    pub workspace_id: String,
    pub working_directory: String,
    pub original_command: Option<String>,
    pub cols: u16,
    pub rows: u16,
    /// Adapter-captured key-value pairs (e.g. session_id for resume).
    #[serde(default)]
    pub adapter_captures: std::collections::HashMap<String, String>,
    /// Which adapter matched (if any).
    pub adapter_id: Option<String>,
    /// True if the terminal was in alternate screen buffer mode when serialized.
    /// TUI apps (vim, htop, Claude Code) use the alternate buffer.
    /// Scrollback from alternate buffer mode is garbled and should not be restored.
    #[serde(default)]
    pub alternate_buffer: bool,
    /// Epoch millis when the scrollback was saved.
    pub saved_at: u64,
}

/// What the frontend sends back when serializing a single pane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScrollbackPayload {
    pub pane_id: String,
    pub session_id: String,
    pub workspace_id: String,
    pub working_directory: String,
    pub original_command: Option<String>,
    pub cols: u16,
    pub rows: u16,
    /// The serialized xterm buffer content.
    pub data: String,
    /// Adapter-captured key-value pairs.
    #[serde(default)]
    pub adapter_captures: std::collections::HashMap<String, String>,
    pub adapter_id: Option<String>,
    /// True if the terminal was in alternate screen buffer mode when serialized.
    #[serde(default)]
    pub alternate_buffer: bool,
}

/// What the frontend receives when restoring a pane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScrollbackRestore {
    pub data: String,
    pub meta: ScrollbackMeta,
}

// ── In-memory scrollback cache ──────────────────────────────────
//
// When a TerminalPane unmounts (tab switch, workspace switch), it caches its
// serialized buffer here. On close, the serialization flow uses cached data
// for unmounted panes and live serialization for mounted panes.

/// Tauri-managed state holding the in-memory scrollback cache.
#[derive(Default, Clone)]
pub struct ScrollbackCache {
    inner: Arc<Mutex<HashMap<String, ScrollbackPayload>>>,
}

impl ScrollbackCache {
    /// Cache a pane's scrollback data. Key is the session_id.
    pub fn put(&self, session_id: &str, payload: ScrollbackPayload) {
        self.inner
            .lock()
            .unwrap()
            .insert(session_id.to_string(), payload);
    }

    /// Remove cached data for a session (e.g. when re-mounting).
    pub fn remove(&self, session_id: &str) {
        self.inner.lock().unwrap().remove(session_id);
    }

    /// Take all cached entries, draining the cache.
    pub fn take_all(&self) -> Vec<ScrollbackPayload> {
        let mut guard = self.inner.lock().unwrap();
        guard.drain().map(|(_, v)| v).collect()
    }

    /// Return the number of cached entries (for testing/diagnostics).
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }
}

// ── Paths ──────────────────────────────────────────────────────

fn scrollback_base() -> PathBuf {
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/share"));
    data_dir.join(crate::APP_DIR_NAME).join("scrollback")
}

fn workspace_dir(workspace_id: &str) -> PathBuf {
    scrollback_base().join(workspace_id)
}

fn data_path(workspace_id: &str, pane_id: &str) -> PathBuf {
    workspace_dir(workspace_id).join(format!("{pane_id}.dat"))
}

fn meta_path(workspace_id: &str, pane_id: &str) -> PathBuf {
    workspace_dir(workspace_id).join(format!("{pane_id}.json"))
}

/// Public accessor for the meta path — used by spawn_pty_for_session to check
/// if a pane has resumable scrollback before the frontend loads it.
pub fn meta_path_for(workspace_id: &str, pane_id: &str) -> PathBuf {
    meta_path(workspace_id, pane_id)
}

// ── Write ──────────────────────────────────────────────────────

/// Save a single pane's scrollback to disk.
pub fn save_scrollback(payload: &ScrollbackPayload) -> Result<(), String> {
    let ws_dir = workspace_dir(&payload.workspace_id);
    fs::create_dir_all(&ws_dir).map_err(|e| format!("Failed to create scrollback dir: {e}"))?;

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let meta = ScrollbackMeta {
        pane_id: payload.pane_id.clone(),
        session_id: payload.session_id.clone(),
        workspace_id: payload.workspace_id.clone(),
        working_directory: payload.working_directory.clone(),
        original_command: payload.original_command.clone(),
        cols: payload.cols,
        rows: payload.rows,
        adapter_captures: payload.adapter_captures.clone(),
        adapter_id: payload.adapter_id.clone(),
        alternate_buffer: payload.alternate_buffer,
        saved_at: now_ms,
    };

    let data_file = data_path(&payload.workspace_id, &payload.pane_id);
    fs::write(&data_file, &payload.data)
        .map_err(|e| format!("Failed to write scrollback data: {e}"))?;

    let meta_file = meta_path(&payload.workspace_id, &payload.pane_id);
    let meta_json = serde_json::to_string_pretty(&meta)
        .map_err(|e| format!("Failed to serialize scrollback meta: {e}"))?;
    fs::write(&meta_file, meta_json)
        .map_err(|e| format!("Failed to write scrollback meta: {e}"))?;

    Ok(())
}

// ── Read ───────────────────────────────────────────────────────

/// Load a single pane's scrollback from disk. Returns None if not found.
pub fn load_scrollback(workspace_id: &str, pane_id: &str) -> Option<ScrollbackRestore> {
    let data_file = data_path(workspace_id, pane_id);
    let meta_file = meta_path(workspace_id, pane_id);

    let data = fs::read_to_string(&data_file).ok()?;
    let meta_json = fs::read_to_string(&meta_file).ok()?;
    let meta: ScrollbackMeta = serde_json::from_str(&meta_json).ok()?;

    Some(ScrollbackRestore { data, meta })
}

/// Find scrollback metadata for a live session by session_id.
/// Scans all workspace scrollback metadata files on disk.
pub fn find_scrollback_meta_for_session(
    session_id: &str,
) -> Option<(String, String, ScrollbackMeta)> {
    let mut wanted = HashSet::new();
    wanted.insert(session_id);
    find_scrollback_meta_for_sessions(&wanted).remove(session_id)
}

/// Batch form of [`find_scrollback_meta_for_session`]: one walk of the
/// scrollback tree resolves every session in `session_ids`, keyed by session
/// id. Hydrating a workspace with N panes used to walk (and parse) every
/// meta file on disk N times — O(sessions × files) reads of the same JSON.
///
/// Same semantics per session as the single-id lookup: directory iteration
/// order, first match wins, unreadable or unparsable files are skipped. The
/// walk stops as soon as every requested id has been found.
pub fn find_scrollback_meta_for_sessions(
    session_ids: &HashSet<&str>,
) -> HashMap<String, (String, String, ScrollbackMeta)> {
    let mut found = HashMap::with_capacity(session_ids.len());
    if session_ids.is_empty() {
        return found;
    }
    let base = scrollback_base();
    let Ok(workspace_entries) = fs::read_dir(&base) else {
        return found;
    };

    for workspace_entry in workspace_entries.flatten() {
        if !workspace_entry
            .file_type()
            .map(|t| t.is_dir())
            .unwrap_or(false)
        {
            continue;
        }

        let workspace_id = workspace_entry.file_name().to_string_lossy().to_string();
        let Ok(entries) = fs::read_dir(workspace_entry.path()) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }

            let Ok(meta_json) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(meta) = serde_json::from_str::<ScrollbackMeta>(&meta_json) else {
                continue;
            };

            if !session_ids.contains(meta.session_id.as_str())
                || found.contains_key(&meta.session_id)
            {
                continue;
            }
            let session_id = meta.session_id.clone();
            let pane_id = meta.pane_id.clone();
            found.insert(session_id, (workspace_id.clone(), pane_id, meta));
            if found.len() == session_ids.len() {
                return found;
            }
        }
    }

    found
}

/// List all pane IDs that have scrollback saved for a workspace.
pub fn list_workspace_scrollbacks(workspace_id: &str) -> Vec<String> {
    let ws_dir = workspace_dir(workspace_id);
    let Ok(entries) = fs::read_dir(&ws_dir) else {
        return vec![];
    };

    entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".dat") {
                Some(name.trim_end_matches(".dat").to_string())
            } else {
                None
            }
        })
        .collect()
}

// ── Cleanup ────────────────────────────────────────────────────

/// Remove scrollback files for a specific workspace.
pub fn remove_workspace_scrollback(workspace_id: &str) {
    let ws_dir = workspace_dir(workspace_id);
    if ws_dir.exists() {
        let _ = fs::remove_dir_all(&ws_dir);
    }
}

/// Remove scrollback for a single pane.
pub fn remove_pane_scrollback(workspace_id: &str, pane_id: &str) {
    let _ = fs::remove_file(data_path(workspace_id, pane_id));
    let _ = fs::remove_file(meta_path(workspace_id, pane_id));
}

/// Remove scrollback directories that don't correspond to any known workspace.
/// Returns the list of removed workspace IDs.
pub fn cleanup_orphan_scrollbacks(active_workspace_ids: &[String]) -> Vec<String> {
    cleanup_orphan_scrollbacks_in(&scrollback_base(), active_workspace_ids)
}

/// Same as `cleanup_orphan_scrollbacks` but reads from a caller-supplied
/// base. Split out so the test can exercise the wiping logic in a
/// private tempdir; without this, the test had to call the shared
/// `scrollback_base()` and races against any peer test mid-write
/// inside `save_scrollback`.
pub(crate) fn cleanup_orphan_scrollbacks_in(
    base: &Path,
    active_workspace_ids: &[String],
) -> Vec<String> {
    let Ok(entries) = fs::read_dir(base) else {
        return vec![];
    };

    let mut removed = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().to_string();
        if !active_workspace_ids.contains(&dir_name) {
            let _ = fs::remove_dir_all(entry.path());
            removed.push(dir_name);
        }
    }
    removed
}

/// Enforce a total disk usage limit for all scrollback files.
/// Removes oldest workspaces first until under the limit.
/// Returns the number of bytes freed.
pub fn enforce_disk_limit(max_bytes: u64) -> u64 {
    let base = scrollback_base();
    if !base.exists() {
        return 0;
    }

    // Collect (workspace_dir, total_size, oldest_mtime)
    let mut workspaces: Vec<(PathBuf, u64, u64)> = Vec::new();

    if let Ok(entries) = fs::read_dir(&base) {
        for entry in entries.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let mut total_size = 0u64;
            let mut oldest_mtime = u64::MAX;

            if let Ok(files) = fs::read_dir(entry.path()) {
                for file in files.flatten() {
                    if let Ok(meta) = file.metadata() {
                        total_size += meta.len();
                        if let Ok(modified) = meta.modified() {
                            let ms = modified
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as u64;
                            oldest_mtime = oldest_mtime.min(ms);
                        }
                    }
                }
            }

            workspaces.push((entry.path(), total_size, oldest_mtime));
        }
    }

    let current_total: u64 = workspaces.iter().map(|(_, s, _)| s).sum();
    if current_total <= max_bytes {
        return 0;
    }

    // Sort oldest first
    workspaces.sort_by_key(|(_, _, mtime)| *mtime);

    let mut freed = 0u64;
    let overshoot = current_total - max_bytes;
    for (path, size, _) in &workspaces {
        if freed >= overshoot {
            break;
        }
        let _ = fs::remove_dir_all(path);
        freed += size;
    }

    freed
}

/// Get total disk usage of all scrollback files in bytes.
pub fn total_disk_usage() -> u64 {
    let base = scrollback_base();
    if !base.exists() {
        return 0;
    }

    let mut total = 0u64;
    if let Ok(entries) = fs::read_dir(&base) {
        for entry in entries.flatten() {
            if let Ok(files) = fs::read_dir(entry.path()) {
                for file in files.flatten() {
                    if let Ok(meta) = file.metadata() {
                        total += meta.len();
                    }
                }
            }
        }
    }
    total
}

// ── Tauri Commands ─────────────────────────────────────────────

/// Enrich a scrollback payload with adapter captures from multiple sources:
/// 1. Pane-level adapter captures (written by hooks during the session)
/// 2. PTY output scanner captures (for tools that print session IDs)
/// 3. Command matching to tag the adapter_id even without captures
fn enrich_with_adapter_captures(
    payload: &mut ScrollbackPayload,
    adapter_state: &crate::session_adapters::AdapterState,
    app_state: &crate::state::AppStateStore,
) {
    // Source 1: pane-level captures (from hook callbacks during the active session).
    let pane_captures = app_state.get_terminal_adapter_captures(&payload.session_id);
    if !pane_captures.is_empty() {
        payload.adapter_captures.extend(pane_captures);
    }

    // Source 2: scanner captures (from PTY output regex matching)
    if let Some((adapter_id, captures)) = adapter_state.get_captures(&payload.session_id) {
        payload.adapter_id = Some(adapter_id);
        payload.adapter_captures.extend(captures);
    } else if payload.original_command.is_some() {
        // No scanner captures. Match against adapter detect_pattern to tag adapter_id.
        let config = adapter_state.config.lock().unwrap();
        for (id, adapter) in &config.adapters {
            if let Ok(re) = regex::Regex::new(&adapter.detect_pattern) {
                if re.is_match(payload.original_command.as_deref().unwrap_or("")) {
                    payload.adapter_id = Some(id.clone());
                    break;
                }
            }
        }
    }
}

/// Refresh scrollback metadata files with fresh adapter captures from the
/// in-memory snapshot.  Called during the close sequence so that sessions whose
/// panes were never mounted (inactive workspaces) still get up-to-date captures
/// written to disk.  Only the metadata sidecar is touched — scrollback content
/// is left as-is.
pub fn refresh_stale_scrollback_metadata(store: &crate::state::AppStateStore) {
    let snapshot = store.snapshot();
    for session in &snapshot.terminal_sessions {
        let captures = &session.adapter_captures;
        if captures.is_empty() {
            continue;
        }
        // Locate the existing metadata file for this session on disk.
        let Some((ws_id, pane_id, mut meta)) =
            find_scrollback_meta_for_session(&session.session_id.0)
        else {
            continue;
        };
        // Merge snapshot captures into the metadata (snapshot wins on conflict).
        let mut changed = false;
        for (key, value) in captures {
            if meta.adapter_captures.get(key) != Some(value) {
                meta.adapter_captures
                    .insert(key.clone(), value.clone());
                changed = true;
            }
        }
        if !changed {
            continue;
        }
        let path = meta_path(&ws_id, &pane_id);
        match serde_json::to_string_pretty(&meta) {
            Ok(json) => {
                if let Err(e) = fs::write(&path, json) {
                    eprintln!(
                        "[codemux::scrollback] Failed to refresh metadata for {}/{}: {e}",
                        ws_id, pane_id
                    );
                }
            }
            Err(e) => {
                eprintln!(
                    "[codemux::scrollback] Failed to serialize refreshed metadata: {e}"
                );
            }
        }
    }
}

#[tauri::command]
pub fn save_terminal_scrollback(
    app_state: tauri::State<'_, crate::state::AppStateStore>,
    adapter_state: tauri::State<'_, crate::session_adapters::AdapterState>,
    mut payload: ScrollbackPayload,
) -> Result<(), String> {
    enrich_with_adapter_captures(&mut payload, &adapter_state, &app_state);
    save_scrollback(&payload)
}

#[tauri::command]
pub fn get_terminal_scrollback(
    workspace_id: String,
    pane_id: String,
) -> Result<Option<ScrollbackRestore>, String> {
    let result = load_scrollback(&workspace_id, &pane_id);
    Ok(result)
}

/// Cache a pane's scrollback in memory (called on TerminalPane unmount).
#[tauri::command]
pub fn cache_terminal_scrollback(
    cache: tauri::State<'_, ScrollbackCache>,
    app_state: tauri::State<'_, crate::state::AppStateStore>,
    adapter_state: tauri::State<'_, crate::session_adapters::AdapterState>,
    mut payload: ScrollbackPayload,
) -> Result<(), String> {
    enrich_with_adapter_captures(&mut payload, &adapter_state, &app_state);
    let session_id = payload.session_id.clone();
    cache.put(&session_id, payload);
    Ok(())
}

/// Remove a pane's cached scrollback (called when TerminalPane re-mounts).
#[tauri::command]
pub fn uncache_terminal_scrollback(
    cache: tauri::State<'_, ScrollbackCache>,
    session_id: String,
) -> Result<(), String> {
    cache.remove(&session_id);
    Ok(())
}

/// Flush all in-memory cached scrollback entries to disk. Returns the
/// number of entries successfully written.
///
/// Used by two callers: the Tauri command `flush_scrollback_cache`
/// (frontend-driven, normal close path) and the Windows backend
/// close-handler backstop in `lib.rs` (added 2026-04-15 to fix the
/// "terminals start fresh on Windows" symptom).
///
/// Idempotent — drains the cache, so calling it twice is safe (the
/// second call returns 0). Empty-data payloads are silently skipped
/// (they're useless to restore from) but still removed from the cache.
///
/// Cross-platform on purpose: the helper itself has no Windows-only
/// behavior. Its second caller (the close-handler backstop) is the
/// thing that's gated to Windows, because that's where the frontend
/// 3-second serialization timeout actually fires in practice. Linux
/// IPC is fast enough that the frontend reliably drains the cache
/// itself before the backend's timeout expires; on Windows slower
/// Tauri IPC + slower xterm serialization can blow the budget and
/// leave cached entries unwritten.
pub fn flush_cache_to_disk(cache: &ScrollbackCache) -> u32 {
    let entries = cache.take_all();
    let mut saved = 0u32;
    for payload in entries {
        if !payload.data.is_empty() {
            if let Err(e) = save_scrollback(&payload) {
                eprintln!("[codemux::scrollback] Failed to flush cached scrollback: {e}");
            } else {
                saved += 1;
            }
        }
    }
    saved
}

/// Flush all cached scrollback entries to disk. Called during the close
/// sequence AFTER live panes have been serialized, so that cached (unmounted)
/// panes also get persisted.
#[tauri::command]
pub fn flush_scrollback_cache(cache: tauri::State<'_, ScrollbackCache>) -> Result<u32, String> {
    Ok(flush_cache_to_disk(&cache))
}

/// Decides whether startup should run `cleanup_orphan_scrollbacks`.
///
/// On Windows, returns false when `layout_loaded` is false. The reason
/// is that `cleanup_orphan_scrollbacks(&[])` (called with an empty
/// active-workspace list, which is what `state.clear_workspaces()`
/// produces) deletes EVERY scrollback dir on disk. Force-close cascade:
///
/// 1. User force-closes Codemux on Windows (e.g. from the historic
///    runaway-spawn freeze, or because the close-handler timeout fired).
/// 2. The `WindowEvent::CloseRequested` handler never runs, so
///    `flush_persisted_state` never runs, so `layout.json` is missing.
/// 3. Next launch: `load_persisted_state()` returns `None`, the setup
///    code calls `state.clear_workspaces()`, the workspace ID list
///    is now empty.
/// 4. The startup cleanup pass calls `cleanup_orphan_scrollbacks(&[])`
///    and wipes every saved scrollback dir on disk.
/// 5. Even after the user fixes the original close issue, every
///    previously-saved terminal is gone forever — terminals "start fresh"
///    on every launch.
///
/// Skipping cleanup when we don't actually know which workspaces are
/// valid preserves recovery state. The tradeoff is that genuinely
/// orphaned scrollback (from workspaces deleted between sessions) won't
/// be cleaned up until a successful load happens — but the
/// `enforce_disk_limit` pass that runs right after still reclaims space
/// if the orphans pile up, so this isn't a leak.
///
/// On Linux/macOS, returns true regardless. The historical behavior is
/// preserved verbatim — the cascade isn't observed on Linux in practice
/// (the close handler is reliable), so the gate is added only where
/// it's needed.
///
/// Implementation note: uses `cfg!(windows)` (a compile-time constant)
/// instead of a `#[cfg(windows)] { ... }` block-form so that
/// `layout_loaded` is syntactically referenced on every platform. The
/// runtime check is constant-folded by the optimizer to the same code
/// either form would produce, but the syntactic reference silences both
/// rustc and rust-analyzer `unused_variables` / `unused_assignments`
/// warnings at the `lib.rs` call site without needing `#[allow]`
/// plumbing on the caller.
pub fn should_run_orphan_cleanup(layout_loaded: bool) -> bool {
    if cfg!(windows) {
        layout_loaded
    } else {
        // Linux/macOS: the historical behavior is preserved verbatim.
        // `layout_loaded` is referenced in the `if cfg!(windows)` arm
        // above, so the compiler sees it as used on every platform.
        true
    }
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn test_payload(ws: &str, pane: &str) -> ScrollbackPayload {
        ScrollbackPayload {
            pane_id: pane.into(),
            session_id: "sess-1".into(),
            workspace_id: ws.into(),
            working_directory: "/tmp".into(),
            original_command: Some("claude --dangerously-skip-permissions".into()),
            cols: 120,
            rows: 40,
            data: "test scrollback content\r\nline 2\r\n".into(),
            adapter_captures: HashMap::new(),
            adapter_id: None,
            alternate_buffer: false,
        }
    }

    #[test]
    fn scrollback_roundtrip() {
        let ws = format!("test-ws-{}", std::process::id());
        let payload = test_payload(&ws, "pane-1");

        save_scrollback(&payload).unwrap();

        let restored = load_scrollback(&ws, "pane-1").unwrap();
        assert_eq!(restored.data, payload.data);
        assert_eq!(restored.meta.pane_id, "pane-1");
        assert_eq!(restored.meta.working_directory, "/tmp");
        assert_eq!(
            restored.meta.original_command.as_deref(),
            Some("claude --dangerously-skip-permissions")
        );
        assert_eq!(restored.meta.cols, 120);

        // Cleanup
        remove_workspace_scrollback(&ws);
        assert!(load_scrollback(&ws, "pane-1").is_none());
    }

    #[test]
    fn list_workspace_scrollbacks_works() {
        let ws = format!("test-ws-list-{}", std::process::id());
        save_scrollback(&test_payload(&ws, "pane-a")).unwrap();
        save_scrollback(&test_payload(&ws, "pane-b")).unwrap();

        let mut ids = list_workspace_scrollbacks(&ws);
        ids.sort();
        assert_eq!(ids, vec!["pane-a", "pane-b"]);

        remove_workspace_scrollback(&ws);
    }

    #[test]
    fn cleanup_orphan_scrollbacks_works() {
        // Drive the wiping logic against a private tempdir, NOT the
        // shared `scrollback_base()`. The previous version called
        // `save_scrollback` (writes to the shared base) followed by
        // `cleanup_orphan_scrollbacks` (wipes everything in the
        // shared base not in the keep list). When tests ran in
        // parallel, the wipe could fire between another test's
        // data-write and meta-write, corrupting that peer mid-flight.
        // Using a tempdir + the `_in` core function isolates the
        // wipe and exercises the same logic.
        let base = tempfile::tempdir().unwrap();
        let ws_keep = "ws-keep-isolated";
        let ws_orphan = "ws-orphan-isolated";
        fs::create_dir_all(base.path().join(ws_keep)).unwrap();
        fs::write(base.path().join(ws_keep).join("pane-1.dat"), b"keep").unwrap();
        fs::create_dir_all(base.path().join(ws_orphan)).unwrap();
        fs::write(base.path().join(ws_orphan).join("pane-1.dat"), b"orphan").unwrap();

        let removed =
            cleanup_orphan_scrollbacks_in(base.path(), &[ws_keep.to_string()]);
        assert!(removed.contains(&ws_orphan.to_string()));

        assert!(base.path().join(ws_keep).exists());
        assert!(!base.path().join(ws_orphan).exists());
    }

    #[test]
    fn missing_scrollback_returns_none() {
        assert!(load_scrollback("nonexistent-ws", "nonexistent-pane").is_none());
    }

    #[test]
    fn corrupted_meta_returns_none() {
        let ws = format!("test-ws-corrupt-{}", std::process::id());
        let payload = test_payload(&ws, "pane-1");
        save_scrollback(&payload).unwrap();

        // Corrupt the meta file
        let meta = meta_path(&ws, "pane-1");
        fs::write(&meta, "not valid json").unwrap();

        assert!(load_scrollback(&ws, "pane-1").is_none());

        remove_workspace_scrollback(&ws);
    }

    #[test]
    fn find_scrollback_meta_for_session_works() {
        let pid = std::process::id();
        let ws = format!("test-ws-find-{pid}");
        let sid = format!("sess-find-{pid}");
        let payload = test_payload_with_session(&ws, "pane-1", &sid);
        save_scrollback(&payload).unwrap();

        let found = find_scrollback_meta_for_session(&sid).unwrap();
        assert_eq!(found.0, payload.workspace_id);
        assert_eq!(found.1, "pane-1");
        assert_eq!(found.2.session_id, sid);

        remove_workspace_scrollback(&ws);
    }

    #[test]
    fn find_scrollback_meta_for_sessions_resolves_a_batch_in_one_walk() {
        let pid = std::process::id();
        let ws_a = format!("test-ws-batch-a-{pid}");
        let ws_b = format!("test-ws-batch-b-{pid}");
        let sid_a = format!("sess-batch-a-{pid}");
        let sid_b = format!("sess-batch-b-{pid}");
        let sid_missing = format!("sess-batch-missing-{pid}");
        save_scrollback(&test_payload_with_session(&ws_a, "pane-a", &sid_a)).unwrap();
        save_scrollback(&test_payload_with_session(&ws_b, "pane-b", &sid_b)).unwrap();
        // A sibling that must not be returned: it was not asked for.
        save_scrollback(&test_payload_with_session(
            &ws_b,
            "pane-other",
            &format!("sess-batch-other-{pid}"),
        ))
        .unwrap();

        let wanted: HashSet<&str> = [sid_a.as_str(), sid_b.as_str(), sid_missing.as_str()]
            .into_iter()
            .collect();
        let found = find_scrollback_meta_for_sessions(&wanted);

        assert_eq!(found.len(), 2);
        let (ws, pane, meta) = &found[&sid_a];
        assert_eq!((ws.as_str(), pane.as_str()), (ws_a.as_str(), "pane-a"));
        assert_eq!(meta.session_id, sid_a);
        let (ws, pane, _) = &found[&sid_b];
        assert_eq!((ws.as_str(), pane.as_str()), (ws_b.as_str(), "pane-b"));
        assert!(!found.contains_key(&sid_missing));

        // Batch and single lookups agree, entry for entry.
        for sid in [&sid_a, &sid_b] {
            let single = find_scrollback_meta_for_session(sid).unwrap();
            let batch = &found[sid.as_str()];
            assert_eq!(single.0, batch.0);
            assert_eq!(single.1, batch.1);
            assert_eq!(single.2.session_id, batch.2.session_id);
        }
        assert!(find_scrollback_meta_for_sessions(&HashSet::new()).is_empty());

        remove_workspace_scrollback(&ws_a);
        remove_workspace_scrollback(&ws_b);
    }

    #[test]
    fn empty_data_roundtrips() {
        let ws = format!("test-ws-empty-{}", std::process::id());
        let mut payload = test_payload(&ws, "pane-1");
        payload.data = String::new();

        save_scrollback(&payload).unwrap();

        let restored = load_scrollback(&ws, "pane-1").unwrap();
        assert!(restored.data.is_empty());
        assert_eq!(restored.meta.pane_id, "pane-1");

        remove_workspace_scrollback(&ws);
    }

    #[test]
    fn ansi_escape_data_roundtrips() {
        let ws = format!("test-ws-ansi-{}", std::process::id());
        let mut payload = test_payload(&ws, "pane-1");
        // Simulate serialized xterm data with ANSI colors
        payload.data = "\x1b[32mgreen text\x1b[0m\r\n\x1b[1;31mbold red\x1b[0m\r\n".into();

        save_scrollback(&payload).unwrap();

        let restored = load_scrollback(&ws, "pane-1").unwrap();
        assert_eq!(restored.data, payload.data);

        remove_workspace_scrollback(&ws);
    }

    #[test]
    fn adapter_captures_roundtrip() {
        let ws = format!("test-ws-adapter-{}", std::process::id());
        let mut payload = test_payload(&ws, "pane-1");
        payload.adapter_id = Some("claude-code".into());
        payload
            .adapter_captures
            .insert("session_id".into(), "abc-123".into());

        save_scrollback(&payload).unwrap();

        let restored = load_scrollback(&ws, "pane-1").unwrap();
        assert_eq!(restored.meta.adapter_id.as_deref(), Some("claude-code"));
        assert_eq!(
            restored.meta.adapter_captures.get("session_id").unwrap(),
            "abc-123"
        );

        remove_workspace_scrollback(&ws);
    }

    #[test]
    fn cache_put_and_take_all() {
        let cache = ScrollbackCache::default();

        let p1 = test_payload("ws-1", "pane-1");
        let p2 = test_payload("ws-1", "pane-2");

        cache.put("sess-1", p1);
        cache.put("sess-2", p2);
        assert_eq!(cache.len(), 2);

        let entries = cache.take_all();
        assert_eq!(entries.len(), 2);
        assert_eq!(cache.len(), 0); // drained
    }

    #[test]
    fn cache_remove_clears_entry() {
        let cache = ScrollbackCache::default();
        cache.put("sess-1", test_payload("ws-1", "pane-1"));
        assert_eq!(cache.len(), 1);

        cache.remove("sess-1");
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn cache_put_overwrites_stale_entry() {
        let cache = ScrollbackCache::default();

        let mut p1 = test_payload("ws-1", "pane-1");
        p1.data = "old data".into();
        cache.put("sess-1", p1);

        let mut p2 = test_payload("ws-1", "pane-1");
        p2.data = "new data".into();
        cache.put("sess-1", p2);

        assert_eq!(cache.len(), 1);
        let entries = cache.take_all();
        assert_eq!(entries[0].data, "new data");
    }

    #[test]
    fn cache_flush_writes_to_disk() {
        let cache = ScrollbackCache::default();
        let ws = format!("test-ws-cacheflush-{}", std::process::id());

        cache.put("sess-1", test_payload(&ws, "pane-1"));
        cache.put("sess-2", test_payload(&ws, "pane-2"));

        // Flush manually (mirrors what flush_scrollback_cache command does)
        let entries = cache.take_all();
        for payload in &entries {
            save_scrollback(payload).unwrap();
        }

        // Both panes should now be on disk
        assert!(load_scrollback(&ws, "pane-1").is_some());
        assert!(load_scrollback(&ws, "pane-2").is_some());

        remove_workspace_scrollback(&ws);
    }

    // ── Bug 3: refresh_stale_scrollback_metadata tests ────────────

    /// Build a test payload with a unique session ID to avoid collisions when
    /// `find_scrollback_meta_for_session` scans all workspace directories.
    fn test_payload_with_session(ws: &str, pane: &str, session_id: &str) -> ScrollbackPayload {
        ScrollbackPayload {
            pane_id: pane.into(),
            session_id: session_id.into(),
            workspace_id: ws.into(),
            working_directory: "/tmp".into(),
            original_command: Some("claude --dangerously-skip-permissions".into()),
            cols: 120,
            rows: 40,
            data: "test scrollback content\r\nline 2\r\n".into(),
            adapter_captures: HashMap::new(),
            adapter_id: None,
            alternate_buffer: false,
        }
    }

    fn store_with_session(ws_id: &str, session_id: &str, captures: HashMap<String, String>) -> crate::state::AppStateStore {
        use crate::state::*;
        let store = AppStateStore::default();
        let mut snap = store.snapshot();
        snap.terminal_sessions.push(TerminalSessionSnapshot {
            session_id: SessionId(session_id.into()),
            title: "test".into(),
            shell: None,
            cwd: "/tmp".into(),
            cols: 120,
            rows: 40,
            state: TerminalSessionState::Ready,
            last_message: None,
            exit_code: None,
            original_command: Some("claude".into()),
            adapter_captures: captures,
        });
        snap.active_workspace_id = WorkspaceId(ws_id.into());
        store.replace_snapshot(snap);
        store
    }

    #[test]
    fn refresh_updates_stale_adapter_captures() {
        let pid = std::process::id();
        let ws = format!("test-ws-rfrsh-upd-{pid}");
        let sid = format!("sess-rfrsh-upd-{pid}");

        // Save scrollback with old capture
        let mut payload = test_payload_with_session(&ws, "pane-1", &sid);
        payload.adapter_captures.insert("claude_session_id".into(), "old-uuid".into());
        save_scrollback(&payload).unwrap();

        // Build store where the snapshot has a fresh capture
        let mut captures = HashMap::new();
        captures.insert("claude_session_id".into(), "new-uuid".into());
        let store = store_with_session(&ws, &sid, captures);

        refresh_stale_scrollback_metadata(&store);

        // Verify metadata on disk was updated
        let restored = load_scrollback(&ws, "pane-1").unwrap();
        assert_eq!(
            restored.meta.adapter_captures.get("claude_session_id").unwrap(),
            "new-uuid"
        );

        remove_workspace_scrollback(&ws);
    }

    #[test]
    fn refresh_keeps_existing_when_snapshot_has_no_captures() {
        let pid = std::process::id();
        let ws = format!("test-ws-rfrsh-keep-{pid}");
        let sid = format!("sess-rfrsh-keep-{pid}");

        // Save scrollback with existing capture
        let mut payload = test_payload_with_session(&ws, "pane-1", &sid);
        payload.adapter_captures.insert("claude_session_id".into(), "old-uuid".into());
        save_scrollback(&payload).unwrap();

        // Store with empty captures — simulates dead PTY
        let store = store_with_session(&ws, &sid, HashMap::new());

        refresh_stale_scrollback_metadata(&store);

        // Old capture should be preserved (not wiped)
        let restored = load_scrollback(&ws, "pane-1").unwrap();
        assert_eq!(
            restored.meta.adapter_captures.get("claude_session_id").unwrap(),
            "old-uuid"
        );

        remove_workspace_scrollback(&ws);
    }

    #[test]
    fn refresh_skips_sessions_without_scrollback_on_disk() {
        // Store has a session but no scrollback file exists — should not crash
        let mut captures = HashMap::new();
        captures.insert("claude_session_id".into(), "uuid".into());
        let store = store_with_session("nonexistent-ws", "sess-no-disk", captures);

        // Should complete without error
        refresh_stale_scrollback_metadata(&store);
    }

    #[test]
    fn refresh_no_write_when_captures_already_match() {
        let pid = std::process::id();
        let ws = format!("test-ws-rfrsh-noop-{pid}");
        let sid = format!("sess-rfrsh-noop-{pid}");

        // Save with captures that already match the snapshot
        let mut payload = test_payload_with_session(&ws, "pane-1", &sid);
        payload.adapter_captures.insert("claude_session_id".into(), "same-uuid".into());
        save_scrollback(&payload).unwrap();

        let original_meta_json = std::fs::read_to_string(meta_path(&ws, "pane-1")).unwrap();
        let original: ScrollbackMeta = serde_json::from_str(&original_meta_json).unwrap();
        let original_saved_at = original.saved_at;

        // Store with identical captures
        let mut captures = HashMap::new();
        captures.insert("claude_session_id".into(), "same-uuid".into());
        let store = store_with_session(&ws, &sid, captures);

        refresh_stale_scrollback_metadata(&store);

        // saved_at should NOT have changed (no write occurred)
        let after_meta_json = std::fs::read_to_string(meta_path(&ws, "pane-1")).unwrap();
        let after: ScrollbackMeta = serde_json::from_str(&after_meta_json).unwrap();
        assert_eq!(after.saved_at, original_saved_at);

        remove_workspace_scrollback(&ws);
    }

    // ── flush_cache_to_disk tests ─────────────────────────────────
    //
    // These test the helper that the Windows close-handler backstop
    // calls in `lib.rs`. The helper itself is cross-platform; the
    // backstop call site is `#[cfg(windows)]`. Tests run on every
    // platform via the cross-platform helper interface.

    #[test]
    fn flush_cache_to_disk_empty_returns_zero() {
        // Idempotency guarantee: calling on an empty cache must not
        // panic and must return 0. This is the post-drain state, so
        // the backstop calling it after the frontend has already
        // drained the cache is a documented no-op.
        let cache = ScrollbackCache::default();
        assert_eq!(flush_cache_to_disk(&cache), 0);
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn flush_cache_to_disk_writes_payloads_to_disk() {
        let pid = std::process::id();
        let ws = format!("test-ws-flushcache-{pid}");
        let cache = ScrollbackCache::default();

        // Two cached entries with non-empty data
        let mut p1 = test_payload(&ws, "pane-fc-1");
        p1.session_id = format!("sess-fc-1-{pid}");
        let mut p2 = test_payload(&ws, "pane-fc-2");
        p2.session_id = format!("sess-fc-2-{pid}");

        let sid1 = p1.session_id.clone();
        let sid2 = p2.session_id.clone();
        cache.put(&sid1, p1);
        cache.put(&sid2, p2);
        assert_eq!(cache.len(), 2);

        // Flush the cache through the helper the Windows backstop calls
        let saved = flush_cache_to_disk(&cache);
        assert_eq!(saved, 2);

        // Cache must be drained
        assert_eq!(cache.len(), 0);

        // Both files must exist on disk
        assert!(load_scrollback(&ws, "pane-fc-1").is_some());
        assert!(load_scrollback(&ws, "pane-fc-2").is_some());

        remove_workspace_scrollback(&ws);
    }

    #[test]
    fn flush_cache_to_disk_drains_so_second_call_returns_zero() {
        // Regression guard: the helper MUST drain the cache. If a
        // future refactor switches to non-draining iteration, the
        // backstop could double-write entries OR (worse) the frontend
        // and backstop could race. Drain semantics keep both callers
        // from stepping on each other.
        let pid = std::process::id();
        let ws = format!("test-ws-drain-{pid}");
        let cache = ScrollbackCache::default();

        let mut p = test_payload(&ws, "pane-drain");
        p.session_id = format!("sess-drain-{pid}");
        let sid = p.session_id.clone();
        cache.put(&sid, p);

        assert_eq!(flush_cache_to_disk(&cache), 1);
        assert_eq!(flush_cache_to_disk(&cache), 0);
        assert_eq!(cache.len(), 0);

        remove_workspace_scrollback(&ws);
    }

    #[test]
    fn flush_cache_to_disk_skips_empty_data_payloads() {
        // Empty-data payloads are useless to restore from (the xterm
        // serialize addon would just produce a blank pane). The
        // helper drains them from the cache but does NOT write them
        // to disk, so they don't count toward the saved tally.
        let pid = std::process::id();
        let ws = format!("test-ws-emptydata-{pid}");
        let cache = ScrollbackCache::default();

        let mut p = test_payload(&ws, "pane-emptydata");
        p.session_id = format!("sess-emptydata-{pid}");
        p.data = String::new();
        let sid = p.session_id.clone();
        cache.put(&sid, p);

        let saved = flush_cache_to_disk(&cache);
        assert_eq!(saved, 0);

        // Cache IS drained even though nothing was written
        assert_eq!(cache.len(), 0);

        // No file on disk
        assert!(load_scrollback(&ws, "pane-emptydata").is_none());
    }

    #[test]
    fn flush_cache_to_disk_mixed_empty_and_nonempty_counts_only_writes() {
        // Verifies the per-entry decision: skip empty, write non-empty,
        // and the returned count reflects only the actual writes.
        let pid = std::process::id();
        let ws = format!("test-ws-mixed-{pid}");
        let cache = ScrollbackCache::default();

        let mut p_empty = test_payload(&ws, "pane-mix-empty");
        p_empty.session_id = format!("sess-mix-empty-{pid}");
        p_empty.data = String::new();

        let mut p_nonempty = test_payload(&ws, "pane-mix-data");
        p_nonempty.session_id = format!("sess-mix-data-{pid}");

        let sid1 = p_empty.session_id.clone();
        let sid2 = p_nonempty.session_id.clone();
        cache.put(&sid1, p_empty);
        cache.put(&sid2, p_nonempty);
        assert_eq!(cache.len(), 2);

        let saved = flush_cache_to_disk(&cache);
        assert_eq!(saved, 1, "only the non-empty payload should count");

        // Both removed from cache
        assert_eq!(cache.len(), 0);

        // Only the non-empty one is on disk
        assert!(load_scrollback(&ws, "pane-mix-empty").is_none());
        assert!(load_scrollback(&ws, "pane-mix-data").is_some());

        remove_workspace_scrollback(&ws);
    }

    // ── should_run_orphan_cleanup gate tests ──────────────────────
    //
    // The Windows fix for "terminals start fresh" closes the
    // force-close cascade by skipping orphan cleanup when the layout
    // file failed to load. Linux preserves historical behavior.

    #[cfg(windows)]
    #[test]
    fn should_run_orphan_cleanup_windows_skips_when_layout_not_loaded() {
        // The whole point of the Windows gate. layout_loaded=false
        // means we don't actually know which workspaces are valid,
        // so cleanup must NOT run — otherwise an empty workspace ID
        // list would wipe every scrollback dir on disk.
        assert!(
            !should_run_orphan_cleanup(false),
            "Windows must skip orphan cleanup when layout was not loaded; \
             otherwise cleanup_orphan_scrollbacks(&[]) wipes everything"
        );
    }

    #[cfg(windows)]
    #[test]
    fn should_run_orphan_cleanup_windows_runs_when_layout_loaded() {
        // Happy path on Windows: layout loaded, workspace IDs are
        // trustworthy, cleanup proceeds normally.
        assert!(
            should_run_orphan_cleanup(true),
            "Windows must run orphan cleanup when layout loaded successfully"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn should_run_orphan_cleanup_unix_always_runs_regardless_of_layout() {
        // Linux/macOS preserves historical behavior. The cascade
        // isn't observed on Linux in practice (close handler is
        // reliable), so cleanup runs unconditionally — same as
        // before this fix.
        assert!(
            should_run_orphan_cleanup(false),
            "Linux must preserve historical behavior — cleanup always runs"
        );
        assert!(
            should_run_orphan_cleanup(true),
            "Linux must preserve historical behavior — cleanup always runs"
        );
    }

    #[test]
    fn should_run_orphan_cleanup_signature_is_cross_platform() {
        // Compile-time guard: the signature must accept a bool on
        // every platform so callers in `lib.rs` can pass `layout_loaded`
        // without a `cfg`-gated wrapper. Both branches must compile.
        let _ = should_run_orphan_cleanup(true);
        let _ = should_run_orphan_cleanup(false);
    }
}
