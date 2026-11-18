// Skills sync engine (Step 10).
//
// Top-level module that ties together:
//
//   - `path_detection` — "is this path syncable?"
//   - `mapping`        — local↔remote_id JSON table on disk
//   - `api_client`     — POST/PUT/GET/DELETE /api/skills wrappers
//
// Skills are stored **server-side** (plaintext columns, protected
// at rest by the database — the same model `user_settings` sync
// uses). There is no client-held encryption key, so single-sign-on
// users sync without ever setting a sync password.
//
// Single public entry point: `SyncEngine::sync_now(token)`. The
// engine is a Tauri-managed singleton. Every call walks the
// pull-then-push cycle:
//
//   1. Pull the full server list.
//   2. For each remote skill: decide whether to write to disk
//      based on conflict-resolution rules.
//   3. Walk all syncable user-scope skill paths on disk.
//   4. For each local SKILL.md: decide whether to push based on
//      mtime vs the mapping's last_synced_at.
//   5. Persist the mapping atomically.
//
// **Concurrency.** `sync_now` takes the engine's outer mutex for
// the duration of the cycle. Concurrent calls serialize at the
// API entry point — the second waits for the first to release.
// This is simpler than a state-machine + cancellation, fine for
// our load (sync triggers from auth-state-changed and from a
// "Sync now" button — neither produces concurrent demand worth
// pipelining). Stage 6 polish can add a "skip if recently synced"
// shortcut if it ever becomes a problem.
//
// **Echo-loop guard.** When pull writes to disk, the file's mtime
// is recorded into `mapping.last_synced_at_millis`. The push
// pipeline skips files whose mtime equals the recorded value
// exactly. This is robust against the file watcher's normal
// fire-on-write behavior because notify won't fire for a file
// whose mtime stays the same — and even if it does, the equality
// check filters it out.

pub mod api_client;
pub mod export;
pub mod mapping;
pub mod path_detection;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use api_client::{SkillUpload, SkillWire};
use mapping::{account_mapping_path, load_mapping, save_mapping, MappingEntry, SkillMapping};
use path_detection::{
    destination_path_after_pull, detect_skill_path, is_skill_dir_name, user_skill_root_for,
};

/// Sync result reported back to the frontend after a `sync_now`.
/// Caller logs it; nothing here is load-bearing for correctness.
#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    pub pushed_count: usize,
    pub pulled_count: usize,
    pub conflict_count: usize,
    pub error_count: usize,
}

/// Snapshot of the engine's current state for the frontend.
///
/// `rename_all` only renames the variant tags; `rename_all_fields` is what
/// makes the variant fields camelCase, which is the shape the frontend reads.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "state")]
pub enum SyncStateSnapshot {
    /// Engine is between syncs.
    Idle {
        /// Unix millis of the last successful sync, or `None` if
        /// none has run since process start.
        last_sync_at_millis: Option<u128>,
    },
    /// A sync is currently in flight.
    Syncing {
        started_at_millis: u128,
    },
    /// The most recent sync failed. The next call to `sync_now`
    /// will retry. The frontend uses this to surface a banner.
    Error {
        last_error: String,
        at_millis: u128,
    },
}

impl Default for SyncStateSnapshot {
    fn default() -> Self {
        Self::Idle {
            last_sync_at_millis: None,
        }
    }
}

/// The engine. Held by Tauri as managed state; cloned via Arc by
/// every command that touches it.
pub struct SyncEngine {
    inner: Mutex<EngineInner>,
}

struct EngineInner {
    state: SyncStateSnapshot,
    home: PathBuf,
    /// Canonical destination root (`~/.codemux/skills/`) used to
    /// short-circuit lookups during pull.
    skills_root: PathBuf,
}

impl SyncEngine {
    /// Construct an engine with a real home dir.
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        Self::with_home(&home)
    }

    /// Test/injection variant. Production callers use `new()`.
    pub fn with_home(home: &Path) -> Self {
        Self {
            inner: Mutex::new(EngineInner {
                state: SyncStateSnapshot::default(),
                home: home.to_path_buf(),
                skills_root: home.join(".codemux/skills"),
            }),
        }
    }

    /// Read-only state snapshot. Cheap; takes the lock briefly.
    pub fn snapshot(&self) -> SyncStateSnapshot {
        self.inner
            .lock()
            .map(|g| g.state.clone())
            .unwrap_or_default()
    }

    /// Run a full pull-then-push sync. Sequential; the lock
    /// serializes concurrent callers (see module-level comment on
    /// concurrency).
    ///
    /// `enumerate_paths` lets the caller inject the syncable
    /// directory list. Production passes
    /// `crate::skills::paths::enumerate_scan_paths(None,
    /// false).user_paths.into_iter().map(|(p, _)| p).collect()`;
    /// tests pass an explicit list pointing at a tempdir.
    pub async fn sync_now(
        &self,
        token: &str,
        account_id: &str,
        enumerate_paths: Vec<PathBuf>,
    ) -> Result<SyncResult, String> {
        let started_millis = now_millis();
        // Mark Syncing.
        {
            let mut guard = self
                .inner
                .lock()
                .map_err(|e| format!("engine lock poisoned: {e}"))?;
            guard.state = SyncStateSnapshot::Syncing {
                started_at_millis: started_millis,
            };
        }

        let result = self.run_cycle(token, account_id, enumerate_paths).await;

        // Update final state.
        {
            let mut guard = self
                .inner
                .lock()
                .map_err(|e| format!("engine lock poisoned: {e}"))?;
            guard.state = match &result {
                Ok(_) => SyncStateSnapshot::Idle {
                    last_sync_at_millis: Some(now_millis()),
                },
                Err(err) => SyncStateSnapshot::Error {
                    last_error: err.clone(),
                    at_millis: now_millis(),
                },
            };
        }

        result
    }

    async fn run_cycle(
        &self,
        token: &str,
        account_id: &str,
        enumerate_paths: Vec<PathBuf>,
    ) -> Result<SyncResult, String> {
        let (home, skills_root) = {
            let guard = self
                .inner
                .lock()
                .map_err(|e| format!("engine lock poisoned: {e}"))?;
            (guard.home.clone(), guard.skills_root.clone())
        };

        let mapping_path = account_mapping_path(&home, account_id);
        let mut mapping = load_mapping(&mapping_path);
        let mut result = SyncResult::default();

        // ── ONE-TIME E2E → server-side migration ──────────────
        // A mapping written by a build that still encrypted skills
        // client-side has `plaintext_migrated == false`. Zero every
        // entry's `last_synced_at_millis` so the push pass below
        // treats every local skill as dirty and re-uploads it as
        // plaintext via PUT to its existing `remote_id` — rewriting
        // the old ciphertext row in place (no duplicate rows, no
        // data loss). Setting the flag now means this runs exactly
        // once; it's persisted in the `save_mapping` at the end.
        if !mapping.plaintext_migrated {
            for entry in mapping.skills.iter_mut() {
                entry.last_synced_at_millis = 0;
            }
            mapping.plaintext_migrated = true;
        }

        let local_files = collect_local_skill_files(&enumerate_paths);
        let mut local = LocalSkills::new(&local_files, &mapping, &home);

        // ── PULL ──────────────────────────────────────────────
        let remote = api_client::list_skills(token).await?;

        for wire in &remote {
            match self.apply_remote_skill(wire, &mut mapping, &mut local, &home, &skills_root) {
                Ok(applied) => {
                    if applied {
                        result.pulled_count += 1;
                    }
                }
                Err(err) => {
                    eprintln!("[skills_sync] pull '{}' failed: {err}", wire.remote_id);
                    result.error_count += 1;
                }
            }
        }

        // Drop mapping entries whose remote_id is no longer on the
        // server. Server is authoritative for deletion.
        let server_ids: std::collections::HashSet<&str> =
            remote.iter().map(|w| w.remote_id.as_str()).collect();
        mapping
            .skills
            .retain(|e| server_ids.contains(e.remote_id.as_str()));

        // ── PUSH ──────────────────────────────────────────────
        // Walk every syncable directory; for each SKILL.md decide
        // whether it needs an update.
        for local in &local_files {
            match self.push_one(local, &mut mapping, token, &home).await {
                Ok(PushOutcome::Pushed) => result.pushed_count += 1,
                Ok(PushOutcome::Skipped) => {}
                Err(err) => {
                    eprintln!("[skills_sync] push '{}' failed: {err}", local.display());
                    result.error_count += 1;
                }
            }
        }

        save_mapping(&mapping_path, &mapping)?;
        Ok(result)
    }

    /// Apply one remote skill to the local filesystem +
    /// mapping. Returns `true` if a write happened.
    fn apply_remote_skill(
        &self,
        wire: &SkillWire,
        mapping: &mut SkillMapping,
        local: &mut LocalSkills,
        home: &Path,
        skills_root: &Path,
    ) -> Result<bool, String> {
        // Skip rows the server couldn't give us a usable name for.
        // This covers legacy ciphertext-only rows that predate the
        // server-side migration and haven't been re-pushed yet —
        // the server returns them with an empty name. They get
        // rewritten in place the next time the owning device pushes.
        if wire.name.is_empty() {
            return Ok(false);
        }
        // The name becomes a directory under the skills root. Another
        // device, an imported export file or the server itself could send
        // `../x` or an absolute path; such a row is not this device's to
        // write.
        if !is_skill_dir_name(&wire.name) {
            eprintln!(
                "[skills_sync] skipping remote skill {} with an unusable name {:?}",
                wire.remote_id, wire.name
            );
            return Ok(false);
        }

        let server_updated_millis = parse_iso_to_millis(&wire.updated_at)
            .ok_or_else(|| format!("invalid server updated_at: {}", wire.updated_at))?;

        // If we have an existing mapping for this remote_id and
        // the server's updated_at hasn't moved, there's nothing
        // to do. This is the common "everything in sync" path.
        let existing = mapping.find_by_remote_id(&wire.remote_id).cloned();
        if let Some(ref e) = existing {
            if e.server_updated_at_millis == server_updated_millis {
                return Ok(false);
            }
        }

        // Names + contents arrive as plaintext from the server.
        let name_plain = wire.name.clone();
        let content_plain = wire.content.clone();

        // A row this mapping has never seen (first sync for the
        // account, or a lost mapping) may still be a skill that lives
        // on this device. Link it to that file rather than writing a
        // second copy that the push pass would then upload as yet
        // another row.
        if existing.is_none() {
            let key = SkillKey::from_wire(wire);
            let pull_dest = destination_path_after_pull(&name_plain, home);
            if let Some(local_path) = local.claim(&key, &pull_dest) {
                // Identical content is already in sync; otherwise the
                // push pass uploads the local file over this row.
                let in_sync = fs::read_to_string(&local_path).is_ok_and(|c| c == content_plain);
                let last_synced_at_millis = if in_sync {
                    file_mtime_millis(&local_path).unwrap_or(0)
                } else {
                    0
                };
                mapping.upsert(MappingEntry {
                    local_path,
                    remote_id: wire.remote_id.clone(),
                    name: name_plain,
                    provider: wire.provider.clone(),
                    scope: wire.scope.clone(),
                    last_synced_at_millis,
                    server_updated_at_millis: server_updated_millis,
                });
                return Ok(false);
            }
            // Another row already owns this device's copy: a duplicate.
            if local.keys.contains(&key) || mapping.find_by_path(&pull_dest).is_some() {
                return Ok(false);
            }
        }

        // Decide the destination path. New entries always land at
        // the canonical `~/.codemux/skills/<name>/SKILL.md`.
        // Existing entries keep their previously-recorded path so
        // a user who manually moved or renamed a synced skill
        // doesn't get a duplicate written elsewhere.
        // A recorded destination is only used while it is still a user
        // skill location sync may write — one saved by an older build
        // could point anywhere. Such a mapping is dropped, its target left
        // untouched; the next pull treats the row as new.
        //
        // Every remote write is also confined, on disk, to the skill root
        // its path is under: a recorded skill whose folder has since become
        // a link to somewhere else is refused rather than written through.
        let saved_root = existing.as_ref().and_then(|e| user_skill_root_for(&e.local_path, home));
        if let Some(e) = &existing {
            if saved_root.is_none()
                || !detect_skill_path(&e.local_path, home).is_some_and(|info| info.is_syncable)
            {
                eprintln!(
                    "[skills_sync] dropping the mapping for remote skill {}: {} is not a skill location sync writes",
                    wire.remote_id,
                    e.local_path.display()
                );
                mapping.remove_by_remote_id(&wire.remote_id);
                return Ok(false);
            }
        }
        let dest = match &existing {
            Some(e) => e.local_path.clone(),
            None => destination_path_after_pull(&name_plain, home),
        };

        // Conflict resolution: write to disk only if (a) we don't
        // have a mapping yet (first-time pull) OR (b) the server's
        // updated_at is strictly newer than what we last saw. Local
        // edits that haven't been pushed yet are NOT clobbered —
        // the push pass below will upload them and the next pull
        // will see the bumped updated_at.
        let should_write = match &existing {
            None => true,
            Some(e) => server_updated_millis > e.server_updated_at_millis,
        };

        if should_write {
            let confine_to = saved_root.as_deref().unwrap_or(skills_root);
            write_skill_to_disk(&dest, &content_plain, Some(confine_to))?;
        }

        // Record/refresh the mapping. `last_synced_at_millis` is
        // the file's mtime AFTER the write so the push pass'
        // echo-guard equality check matches.
        let last_synced = if should_write {
            file_mtime_millis(&dest).unwrap_or_else(now_millis)
        } else {
            existing
                .as_ref()
                .map(|e| e.last_synced_at_millis)
                .unwrap_or_else(now_millis)
        };

        // Provider stays the recorded one if we have it; for first
        // pulls it's whatever the server tagged.
        let provider = existing
            .as_ref()
            .map(|e| e.provider.clone())
            .unwrap_or_else(|| wire.provider.clone());
        let scope = existing
            .as_ref()
            .map(|e| e.scope.clone())
            .unwrap_or_else(|| wire.scope.clone());

        mapping.upsert(MappingEntry {
            local_path: dest.clone(),
            remote_id: wire.remote_id.clone(),
            name: name_plain,
            provider,
            scope,
            last_synced_at_millis: last_synced,
            server_updated_at_millis: server_updated_millis,
        });

        // Silence the "unused variable" lint for `skills_root` —
        // it lives here for future use (Stage 6 might want to
        // verify `dest` is under `skills_root` for the canonical
        // path enforcement).
        let _ = skills_root;

        Ok(should_write)
    }

    /// Decide whether to push a single local SKILL.md and execute
    /// the push if so.
    async fn push_one(
        &self,
        local: &Path,
        mapping: &mut SkillMapping,
        token: &str,
        home: &Path,
    ) -> Result<PushOutcome, String> {
        // Path classification first — drop anything plugin /
        // project / unknown without reading the file.
        let path_info = match detect_skill_path(local, home) {
            Some(info) if info.is_syncable => info,
            _ => return Ok(PushOutcome::Skipped),
        };

        let mtime_millis = file_mtime_millis(local)
            .ok_or_else(|| format!("could not read mtime of {}", local.display()))?;

        // Echo-loop guard: if the file's mtime exactly matches
        // what we recorded after the most recent write, skip.
        if let Some(existing) = mapping.find_by_path(local) {
            if existing.last_synced_at_millis == mtime_millis {
                return Ok(PushOutcome::Skipped);
            }
        }

        let content = fs::read_to_string(local)
            .map_err(|e| format!("read {}: {e}", local.display()))?;
        let name = skill_name_from_path(local)
            .ok_or_else(|| format!("could not derive skill name from {}", local.display()))?;
        // Every other device would refuse this name on pull, so it is not
        // uploaded as if it would sync.
        if !is_skill_dir_name(&name) {
            eprintln!(
                "[skills_sync] not syncing {}: other devices cannot use the name {name:?}",
                local.display()
            );
            return Ok(PushOutcome::Skipped);
        }

        // Use the mapping's recorded provider when available so
        // origin survives across receiving-device edits (where the
        // file ends up under `~/.codemux/skills/` regardless of
        // which provider it came from).
        let existing = mapping.find_by_path(local).cloned();
        let provider = existing
            .as_ref()
            .map(|e| e.provider.clone())
            .unwrap_or(path_info.provider);
        let scope = existing
            .as_ref()
            .map(|e| e.scope.clone())
            .unwrap_or(path_info.scope);

        let upload = SkillUpload {
            name: name.clone(),
            content,
            provider: provider.clone(),
            scope: scope.clone(),
        };

        let row = match &existing {
            Some(e) => match api_client::update_skill(token, &e.remote_id, &upload).await {
                Ok(r) => r,
                Err(err) if err.contains("HTTP 404") => {
                    // Server doesn't know about this remote_id
                    // (wiped by another device); fall back to
                    // create.
                    mapping.remove_by_remote_id(&e.remote_id);
                    api_client::create_skill(token, &upload).await?
                }
                Err(err) => return Err(err),
            },
            None => api_client::create_skill(token, &upload).await?,
        };

        let server_updated_millis = parse_iso_to_millis(&row.updated_at)
            .ok_or_else(|| format!("server returned invalid updated_at: {}", row.updated_at))?;

        // Re-stat after upload so the recorded mtime reflects the
        // file we actually pushed (covers the race where the file
        // changed between read and stat).
        let recorded_mtime = file_mtime_millis(local).unwrap_or(mtime_millis);

        mapping.upsert(MappingEntry {
            local_path: local.to_path_buf(),
            remote_id: row.remote_id,
            name,
            provider,
            scope,
            last_synced_at_millis: recorded_mtime,
            server_updated_at_millis: server_updated_millis,
        });

        Ok(PushOutcome::Pushed)
    }
}

impl Default for SyncEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
enum PushOutcome {
    Pushed,
    Skipped,
}

/// Identity of a skill across the sync boundary.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SkillKey {
    name: String,
    provider: String,
    scope: String,
}

impl SkillKey {
    fn from_wire(wire: &SkillWire) -> Self {
        Self {
            name: wire.name.clone(),
            provider: wire.provider.clone(),
            scope: wire.scope.clone(),
        }
    }
}

/// Syncable skills on this device, used to link unmapped server rows
/// to existing files instead of duplicating them.
#[derive(Default)]
struct LocalSkills {
    /// Files no mapping entry points at yet, in scan order.
    unclaimed: Vec<(PathBuf, SkillKey)>,
    /// Every syncable skill present on this device.
    keys: HashSet<SkillKey>,
}

impl LocalSkills {
    fn new(files: &[PathBuf], mapping: &SkillMapping, home: &Path) -> Self {
        let mut local = Self::default();
        for path in files {
            let (Some(info), Some(name)) = (detect_skill_path(path, home), skill_name_from_path(path))
            else {
                continue;
            };
            if !info.is_syncable {
                continue;
            }
            let key = SkillKey {
                name,
                provider: info.provider,
                scope: info.scope,
            };
            local.keys.insert(key.clone());
            if mapping.find_by_path(path).is_none() {
                local.unclaimed.push((path.clone(), key));
            }
        }
        local
    }

    /// Take the unclaimed file for `key`, else an unclaimed file at the
    /// row's pull destination (a copy pulled before the mapping was lost).
    fn claim(&mut self, key: &SkillKey, pull_dest: &Path) -> Option<PathBuf> {
        let index = self
            .unclaimed
            .iter()
            .position(|(_, k)| k == key)
            .or_else(|| self.unclaimed.iter().position(|(path, _)| path == pull_dest))?;
        Some(self.unclaimed.remove(index).0)
    }
}

// ────────────────────────────────────────────────────────────────
// Helpers.

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn file_mtime_millis(path: &Path) -> Option<u128> {
    let meta = fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    mtime.duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis())
}

fn parse_iso_to_millis(iso: &str) -> Option<u128> {
    chrono::DateTime::parse_from_rfc3339(iso)
        .ok()
        .map(|dt| dt.timestamp_millis() as u128)
}

/// Derive the skill name from a SKILL.md path: the parent
/// directory's last component is the name. Returns None for
/// malformed paths (no parent, root parent, etc.).
fn skill_name_from_path(path: &Path) -> Option<String> {
    let parent = path.parent()?;
    let name = parent.file_name()?.to_string_lossy().into_owned();
    if name.is_empty() {
        return None;
    }
    Some(name)
}

/// Walk the given root directories and collect every
/// `<root>/<name>/SKILL.md` path. Used by the push pipeline; the
/// mapping table keys off these paths so they must be canonical.
fn collect_local_skill_files(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in roots {
        let read = match fs::read_dir(root) {
            Ok(r) => r,
            Err(_) => continue, // missing root is normal
        };
        for entry in read.flatten() {
            let dir_path = entry.path();
            if !dir_path.is_dir() {
                continue;
            }
            let candidate = dir_path.join("SKILL.md");
            if candidate.is_file() {
                out.push(candidate);
            }
        }
    }
    out
}

/// Write `content` to `dest` atomically.
///
/// With `confine_to`, `dest` must be `<root>/<name>/SKILL.md`, and the write
/// is confined to that folder on disk: a folder that is, or becomes, a link
/// to somewhere else is refused, never written through (see
/// [`confined::write`]).
fn write_skill_to_disk(dest: &Path, content: &str, confine_to: Option<&Path>) -> Result<(), String> {
    let Some(root) = confine_to else {
        return write_unconfined(dest, content);
    };
    let parent = dest
        .parent()
        .ok_or_else(|| format!("skill path has no folder: {}", dest.display()))?;
    let (Some(name), Some(file)) = (parent.file_name(), dest.file_name()) else {
        return Err(format!("{} is not a skill file", dest.display()));
    };
    if parent.parent() != Some(root) || file != "SKILL.md" {
        return Err(format!("{} is not a skill folder under {}", dest.display(), root.display()));
    }
    fs::create_dir_all(root).map_err(|e| format!("create skills root: {e}"))?;
    confined::write(root, name, content.as_bytes())
        .map_err(|e| format!("refusing to write {}: {e}", dest.display()))
}

/// The plain atomic write, for callers that do not need confinement.
fn write_unconfined(dest: &Path, content: &str) -> Result<(), String> {
    use std::io::Write;
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create skill dir: {e}"))?;
    }
    let tmp = dest.with_extension("md.tmp");
    match fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("clear tmp skill: {e}")),
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|e| format!("write tmp skill: {e}"))?;
    file.write_all(content.as_bytes()).map_err(|e| format!("write tmp skill: {e}"))?;
    drop(file);
    fs::rename(&tmp, dest).map_err(|e| format!("rename skill: {e}"))?;
    Ok(())
}

/// Writing `<root>/<name>/SKILL.md` so that the folder checked is the
/// folder written, even if something swaps it for a link mid-write.
mod confined {
    use std::ffi::OsStr;
    use std::path::Path;

    const FILE: &str = "SKILL.md";
    const TMP: &str = "SKILL.md.tmp";

    /// Unix: every step after opening the root goes through file
    /// descriptors. The skill folder is opened with `O_NOFOLLOW` relative
    /// to the root, and the temporary file is created, written and renamed
    /// relative to that folder, so no path is resolved again. std has no
    /// `*at` calls, hence the libc FFI.
    #[cfg(unix)]
    pub(super) fn write(root: &Path, name: &OsStr, content: &[u8]) -> Result<(), String> {
        use std::ffi::CString;
        use std::io::{Error, ErrorKind, Write};
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        use std::os::unix::ffi::OsStrExt;

        let c = |bytes: &[u8]| CString::new(bytes).map_err(|_| "a name contains NUL".to_string());
        let fd = |raw: libc::c_int, what: &str| -> Result<OwnedFd, String> {
            if raw < 0 {
                Err(format!("{what} ({})", Error::last_os_error()))
            } else {
                // SAFETY: `raw` is a descriptor the call just returned, owned
                // by nothing else.
                Ok(unsafe { OwnedFd::from_raw_fd(raw) })
            }
        };
        let dir_flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC;

        let root_c = c(root.as_os_str().as_bytes())?;
        // The root itself may be a link (a dotfiles checkout, say); it is the
        // trusted location.
        // SAFETY: a valid NUL-terminated path and plain flags.
        let root_fd = fd(unsafe { libc::open(root_c.as_ptr(), dir_flags) }, "open the skills root")?;

        let name_c = c(name.as_bytes())?;
        // SAFETY: a live descriptor and a NUL-terminated name.
        if unsafe { libc::mkdirat(root_fd.as_raw_fd(), name_c.as_ptr(), 0o755) } != 0 {
            let error = Error::last_os_error();
            if error.kind() != ErrorKind::AlreadyExists {
                return Err(format!("create the skill folder ({error})"));
            }
        }
        // SAFETY: as above; `O_NOFOLLOW` refuses a link in place of the folder.
        let dir_fd = fd(
            unsafe { libc::openat(root_fd.as_raw_fd(), name_c.as_ptr(), dir_flags | libc::O_NOFOLLOW) },
            "the skill folder is a link or not a folder",
        )?;

        let tmp_c = c(TMP.as_bytes())?;
        let file_c = c(FILE.as_bytes())?;
        // A leftover temporary file is removed, never followed.
        // SAFETY: a live descriptor and a NUL-terminated name.
        if unsafe { libc::unlinkat(dir_fd.as_raw_fd(), tmp_c.as_ptr(), 0) } != 0 {
            let error = Error::last_os_error();
            if error.kind() != ErrorKind::NotFound {
                return Err(format!("clear the temporary file ({error})"));
            }
        }
        // SAFETY: as above; `O_EXCL | O_NOFOLLOW` only ever creates a new
        // regular file.
        let tmp_fd = fd(
            unsafe {
                libc::openat(
                    dir_fd.as_raw_fd(),
                    tmp_c.as_ptr(),
                    libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    0o644 as libc::c_uint,
                )
            },
            "create the temporary file",
        )?;
        let mut tmp = std::fs::File::from(tmp_fd);
        tmp.write_all(content).map_err(|e| format!("write the temporary file ({e})"))?;
        drop(tmp);
        // SAFETY: both names are relative to the same live descriptor.
        if unsafe { libc::renameat(dir_fd.as_raw_fd(), tmp_c.as_ptr(), dir_fd.as_raw_fd(), file_c.as_ptr()) } != 0 {
            return Err(format!("rename the skill file ({})", Error::last_os_error()));
        }
        Ok(())
    }

    /// Windows: the skill folder is held open, refusing a reparse point (a
    /// link or junction) and without sharing delete access, so it cannot be
    /// renamed, removed or replaced while the write runs. Nothing after that
    /// resolves the root's path again:
    /// - the temporary file is created under the held folder's own resolved
    ///   path, and its handle is checked to really sit in that folder;
    /// - the content is written through that handle;
    /// - the rename into `SKILL.md` is made relative to the held folder's
    ///   handle (`FILE_RENAME_INFO.RootDirectory`), not to a path.
    #[cfg(windows)]
    pub(super) fn write(root: &Path, name: &OsStr, content: &[u8]) -> Result<(), String> {
        use std::ffi::OsString;
        use std::fs;
        use std::io::{Error, ErrorKind, Write};
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        use std::os::windows::io::AsRawHandle;
        use std::path::PathBuf;
        use windows_sys::Win32::Storage::FileSystem::{
            FileDispositionInfo, FileRenameInfo, GetFinalPathNameByHandleW, SetFileInformationByHandle,
            DELETE, FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_NAME_NORMALIZED,
            FILE_RENAME_INFO, FILE_RENAME_INFO_0, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE,
            VOLUME_NAME_DOS,
        };

        /// Where an open file really is, links and junctions resolved.
        fn final_path(file: &fs::File) -> Result<PathBuf, String> {
            let mut buf = vec![0u16; 512];
            loop {
                // SAFETY: a live handle and a buffer of the stated length.
                let len = unsafe {
                    GetFinalPathNameByHandleW(
                        file.as_raw_handle(),
                        buf.as_mut_ptr(),
                        buf.len() as u32,
                        FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
                    )
                } as usize;
                if len == 0 {
                    return Err(format!("resolve a path ({})", Error::last_os_error()));
                }
                if len < buf.len() {
                    buf.truncate(len);
                    return Ok(PathBuf::from(OsString::from_wide(&buf)));
                }
                buf.resize(len + 1, 0);
            }
        }

        /// Delete a file through its own handle.
        fn discard(file: &fs::File) {
            let info = FILE_DISPOSITION_INFO { DeleteFile: 1 };
            // SAFETY: a live handle opened with DELETE access, and a struct of
            // the stated size.
            unsafe {
                SetFileInformationByHandle(
                    file.as_raw_handle(),
                    FileDispositionInfo,
                    std::ptr::addr_of!(info).cast(),
                    std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
                );
            }
        }

        let dir = root.join(name);
        match fs::create_dir(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("create the skill folder ({e})")),
        }
        let held = fs::OpenOptions::new()
            .read(true)
            .access_mode(FILE_GENERIC_READ | FILE_TRAVERSE)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&dir)
            .map_err(|e| format!("open the skill folder ({e})"))?;
        let meta = held.metadata().map_err(|e| format!("inspect the skill folder ({e})"))?;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !meta.is_dir() {
            return Err("the skill folder is a link or not a folder".into());
        }
        let held_path = final_path(&held)?;
        let root_path = fs::canonicalize(root).map_err(|e| format!("resolve the skills root ({e})"))?;
        if held_path.parent() != Some(root_path.as_path()) {
            return Err("the skill folder is not inside the skills root".into());
        }

        let tmp_path = held_path.join(TMP);
        match fs::remove_file(&tmp_path) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(format!("clear the temporary file ({e})")),
        }
        let mut tmp = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .access_mode(FILE_GENERIC_WRITE | DELETE)
            .share_mode(0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&tmp_path)
            .map_err(|e| format!("create the temporary file ({e})"))?;
        let landed = final_path(&tmp);
        if landed.as_ref().ok().and_then(|p| p.parent()) != Some(held_path.as_path()) {
            discard(&tmp);
            return Err("the temporary file did not land in the skill folder".into());
        }
        if let Err(e) = tmp.write_all(content).and_then(|()| tmp.flush()) {
            discard(&tmp);
            return Err(format!("write the temporary file ({e})"));
        }

        // FILE_RENAME_INFO ends in a variable-length name; build it in an
        // 8-byte-aligned buffer large enough for the struct plus the name.
        let wide: Vec<u16> = OsStr::new(FILE).encode_wide().collect();
        let size = std::mem::size_of::<FILE_RENAME_INFO>() + wide.len() * 2;
        let mut buf = vec![0u64; size.div_ceil(8)];
        let info = buf.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        // SAFETY: `buf` is zeroed, aligned and large enough for the struct and
        // the name written after its first element.
        let renamed = unsafe {
            (*info).Anonymous = FILE_RENAME_INFO_0 { ReplaceIfExists: 1 };
            (*info).RootDirectory = held.as_raw_handle();
            (*info).FileNameLength = (wide.len() * 2) as u32;
            std::ptr::copy_nonoverlapping(
                wide.as_ptr(),
                std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
                wide.len(),
            );
            SetFileInformationByHandle(tmp.as_raw_handle(), FileRenameInfo, info.cast(), size as u32)
        };
        if renamed == 0 {
            let error = Error::last_os_error();
            discard(&tmp);
            return Err(format!("rename the skill file ({error})"));
        }
        drop(tmp);
        drop(held);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    // Helper to build a plaintext `SkillWire` the way the server
    // now returns them (name + content in the clear).
    fn plain_wire(
        name: &str,
        content: &str,
        provider: &str,
        scope: &str,
        remote_id: &str,
        updated_at_iso: &str,
    ) -> SkillWire {
        SkillWire {
            remote_id: remote_id.into(),
            name: name.into(),
            content: content.into(),
            provider: provider.into(),
            scope: scope.into(),
            updated_at: updated_at_iso.into(),
        }
    }

    // ── SyncStateSnapshot wire shape ───────────────────────────

    #[test]
    fn sync_state_snapshot_serializes_camel_case_fields() {
        let idle = serde_json::to_value(SyncStateSnapshot::Idle {
            last_sync_at_millis: Some(1_700_000_000_000),
        })
        .unwrap();
        assert_eq!(
            idle,
            serde_json::json!({ "state": "idle", "lastSyncAtMillis": 1_700_000_000_000u64 })
        );

        let never = serde_json::to_value(SyncStateSnapshot::default()).unwrap();
        assert_eq!(
            never,
            serde_json::json!({ "state": "idle", "lastSyncAtMillis": null })
        );

        let syncing =
            serde_json::to_value(SyncStateSnapshot::Syncing { started_at_millis: 5 }).unwrap();
        assert_eq!(
            syncing,
            serde_json::json!({ "state": "syncing", "startedAtMillis": 5 })
        );

        let error = serde_json::to_value(SyncStateSnapshot::Error {
            last_error: "boom".into(),
            at_millis: 7,
        })
        .unwrap();
        assert_eq!(
            error,
            serde_json::json!({ "state": "error", "lastError": "boom", "atMillis": 7 })
        );
    }

    // ── skill_name_from_path ───────────────────────────────────

    #[test]
    fn skill_name_from_standard_path() {
        let p = PathBuf::from("/home/u/.codemux/skills/my-skill/SKILL.md");
        assert_eq!(skill_name_from_path(&p).unwrap(), "my-skill");
    }

    #[test]
    fn skill_name_returns_none_for_root() {
        assert!(skill_name_from_path(&PathBuf::from("/")).is_none());
    }

    // ── now_millis / parse_iso ─────────────────────────────────

    #[test]
    fn parse_iso_roundtrip_returns_a_real_millis() {
        let m = parse_iso_to_millis("2026-04-29T20:00:00Z").unwrap();
        assert!(m > 1_000_000_000_000);
    }

    #[test]
    fn parse_iso_returns_none_for_garbage() {
        assert!(parse_iso_to_millis("not a timestamp").is_none());
    }

    // ── collect_local_skill_files ──────────────────────────────

    #[test]
    fn collect_walks_each_root_and_picks_up_skill_md() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join(".codemux/skills");
        fs::create_dir_all(root.join("a")).unwrap();
        fs::create_dir_all(root.join("b")).unwrap();
        fs::write(root.join("a/SKILL.md"), "alpha").unwrap();
        fs::write(root.join("b/SKILL.md"), "beta").unwrap();
        // `c` exists but has no SKILL.md — must be skipped.
        fs::create_dir_all(root.join("c")).unwrap();

        let files = collect_local_skill_files(&[root.clone()]);
        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|p| p.ends_with("a/SKILL.md")));
        assert!(files.iter().any(|p| p.ends_with("b/SKILL.md")));
    }

    #[test]
    fn collect_handles_missing_root_silently() {
        let dir = TempDir::new().unwrap();
        let nonexistent = dir.path().join("nope");
        let files = collect_local_skill_files(&[nonexistent]);
        assert!(files.is_empty());
    }

    // ── apply_remote_skill: pull-write happy path ──────────────

    #[test]
    fn apply_remote_skill_writes_file_for_first_seen_skill() {
        let home = TempDir::new().unwrap();
        let engine = SyncEngine::with_home(home.path());
        let mut mapping = SkillMapping::default();

        let wire = plain_wire(
            "demo",
            "# Demo skill\n\nbody",
            "codemux",
            "user",
            "1",
            "2026-04-29T20:00:00Z",
        );

        let applied = engine
            .apply_remote_skill(
                &wire,
                &mut mapping,
                &mut LocalSkills::default(),
                home.path(),
                &home.path().join(".codemux/skills"),
            )
            .unwrap();
        assert!(applied);

        let dest = destination_path_after_pull("demo", home.path());
        assert!(dest.exists());
        assert_eq!(fs::read_to_string(&dest).unwrap(), "# Demo skill\n\nbody");
        assert_eq!(mapping.skills.len(), 1);
        assert_eq!(mapping.skills[0].remote_id, "1");
        assert_eq!(mapping.skills[0].name, "demo");
        assert_eq!(mapping.skills[0].local_path, dest);
    }

    #[test]
    fn apply_remote_skill_never_writes_outside_the_skills_root() {
        let home = TempDir::new().unwrap();
        let engine = SyncEngine::with_home(home.path());
        let mut mapping = SkillMapping::default();

        for (id, name) in [("1", "../../escaped"), ("2", "/tmp/escaped-abs"), ("3", "..")] {
            let wire = plain_wire(name, "pwned", "codemux", "user", id, "2026-04-29T20:00:00Z");
            let applied = engine
                .apply_remote_skill(
                    &wire,
                    &mut mapping,
                    &mut LocalSkills::default(),
                    home.path(),
                    &home.path().join(".codemux/skills"),
                )
                .unwrap();
            assert!(!applied, "{name:?} must not be written");
        }
        assert!(mapping.skills.is_empty());
        assert!(!home.path().join("escaped").exists());
        assert!(!home.path().join(".codemux/escaped").exists());
        assert!(!std::path::Path::new("/tmp/escaped-abs/SKILL.md").exists());
    }

    #[test]
    fn apply_remote_skill_skips_when_server_updated_at_unchanged() {
        let home = TempDir::new().unwrap();
        let engine = SyncEngine::with_home(home.path());

        // Seed mapping with a stale entry.
        let mut mapping = SkillMapping::default();
        let pre_existing_path = destination_path_after_pull("demo", home.path());
        let server_millis = parse_iso_to_millis("2026-04-29T20:00:00Z").unwrap();
        mapping.upsert(MappingEntry {
            local_path: pre_existing_path.clone(),
            remote_id: "1".into(),
            name: "demo".into(),
            provider: "codemux".into(),
            scope: "user".into(),
            last_synced_at_millis: 0,
            server_updated_at_millis: server_millis,
        });

        // Server returns a wire with the same updated_at — nothing
        // should be written.
        let wire = plain_wire(
            "demo",
            "ignored",
            "codemux",
            "user",
            "1",
            "2026-04-29T20:00:00Z",
        );

        let applied = engine
            .apply_remote_skill(
                &wire,
                &mut mapping,
                &mut LocalSkills::default(),
                home.path(),
                &home.path().join(".codemux/skills"),
            )
            .unwrap();
        assert!(!applied, "no write when server hasn't changed");
        assert!(!pre_existing_path.exists());
    }

    #[test]
    fn a_saved_mapping_outside_the_skill_locations_is_dropped_not_written() {
        let home = TempDir::new().unwrap();
        let engine = SyncEngine::with_home(home.path());
        // Recorded by an older build from an escaping name.
        let escaped = home.path().join(".codemux/skills/../../escaped/SKILL.md");
        let mut mapping = SkillMapping::default();
        mapping.upsert(MappingEntry {
            local_path: escaped,
            remote_id: "1".into(),
            name: "x".into(),
            provider: "codemux".into(),
            scope: "user".into(),
            last_synced_at_millis: 0,
            server_updated_at_millis: 0,
        });

        // A newer, valid row for the same remote id.
        let wire = plain_wire("fine", "body", "codemux", "user", "1", "2026-04-29T20:00:00Z");
        let applied = engine
            .apply_remote_skill(
                &wire,
                &mut mapping,
                &mut LocalSkills::default(),
                home.path(),
                &home.path().join(".codemux/skills"),
            )
            .unwrap();
        assert!(!applied);
        assert!(mapping.skills.is_empty(), "the unsafe mapping is dropped");
        assert!(!home.path().parent().unwrap().join("escaped").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_new_skill_is_never_written_through_a_linked_folder() {
        let home = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let engine = SyncEngine::with_home(home.path());
        let skills_root = home.path().join(".codemux/skills");
        fs::create_dir_all(&skills_root).unwrap();
        std::os::unix::fs::symlink(outside.path(), skills_root.join("linked")).unwrap();

        let wire = plain_wire("linked", "pwned", "codemux", "user", "1", "2026-04-29T20:00:00Z");
        let result = engine.apply_remote_skill(
            &wire,
            &mut SkillMapping::default(),
            &mut LocalSkills::default(),
            home.path(),
            &skills_root,
        );
        assert!(result.is_err(), "the write must be refused");
        assert!(fs::read_dir(outside.path()).unwrap().next().is_none(), "nothing lands outside");
    }

    #[cfg(unix)]
    #[test]
    fn an_update_is_never_written_through_a_saved_skill_folder_that_became_a_link() {
        let home = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let engine = SyncEngine::with_home(home.path());
        let claude_root = home.path().join(".claude/skills");
        fs::create_dir_all(&claude_root).unwrap();
        std::os::unix::fs::symlink(outside.path(), claude_root.join("demo")).unwrap();
        let mut mapping = SkillMapping::default();
        mapping.upsert(MappingEntry {
            local_path: claude_root.join("demo/SKILL.md"),
            remote_id: "1".into(),
            name: "demo".into(),
            provider: "claude".into(),
            scope: "user".into(),
            last_synced_at_millis: 0,
            server_updated_at_millis: 0,
        });

        let wire = plain_wire("demo", "pwned", "claude", "user", "1", "2026-04-29T20:00:00Z");
        let result = engine.apply_remote_skill(
            &wire,
            &mut mapping,
            &mut LocalSkills::default(),
            home.path(),
            &home.path().join(".codemux/skills"),
        );
        assert!(result.is_err(), "the write must be refused");
        assert!(fs::read_dir(outside.path()).unwrap().next().is_none(), "nothing lands outside");
    }

    #[test]
    fn an_update_to_another_providers_saved_skill_still_lands() {
        let home = TempDir::new().unwrap();
        let engine = SyncEngine::with_home(home.path());
        let path = home.path().join(".claude/skills/demo/SKILL.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "old").unwrap();
        let mut mapping = SkillMapping::default();
        mapping.upsert(MappingEntry {
            local_path: path.clone(),
            remote_id: "1".into(),
            name: "demo".into(),
            provider: "claude".into(),
            scope: "user".into(),
            last_synced_at_millis: 0,
            server_updated_at_millis: 0,
        });

        let wire = plain_wire("demo", "new", "claude", "user", "1", "2026-04-29T20:00:00Z");
        let applied = engine
            .apply_remote_skill(
                &wire,
                &mut mapping,
                &mut LocalSkills::default(),
                home.path(),
                &home.path().join(".codemux/skills"),
            )
            .unwrap();
        assert!(applied);
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_link_in_place_of_a_skill_folder_creates_nothing() {
        let home = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let skills_root = home.path().join(".codemux/skills");
        fs::create_dir_all(&skills_root).unwrap();
        let gone = outside.path().join("not-yet");
        std::os::unix::fs::symlink(&gone, skills_root.join("demo")).unwrap();

        let dest = skills_root.join("demo/SKILL.md");
        assert!(write_skill_to_disk(&dest, "pwned", Some(&skills_root)).is_err());
        assert!(!gone.exists(), "no folder is created through the link");
    }

    #[cfg(unix)]
    #[test]
    fn a_confined_write_never_follows_a_link_at_the_temporary_name() {
        let home = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let target = outside.path().join("victim");
        fs::write(&target, "original").unwrap();
        let skills_root = home.path().join(".codemux/skills");
        let dest = skills_root.join("demo/SKILL.md");
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, dest.with_extension("md.tmp")).unwrap();

        write_skill_to_disk(&dest, "new body", Some(&skills_root)).unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "new body");
        assert_eq!(fs::read_to_string(&target).unwrap(), "original");
        assert!(!dest.with_extension("md.tmp").exists());
    }

    // Windows: a junction needs no special privilege, unlike a symlink.
    #[cfg(windows)]
    #[test]
    fn a_new_skill_is_never_written_through_a_junction() {
        let home = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let skills_root = home.path().join(".codemux/skills");
        fs::create_dir_all(&skills_root).unwrap();
        let link = skills_root.join("linked");
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(outside.path())
            .output()
            .unwrap();
        assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));

        let dest = link.join("SKILL.md");
        assert!(write_skill_to_disk(&dest, "pwned", Some(&skills_root)).is_err());
        assert!(fs::read_dir(outside.path()).unwrap().next().is_none(), "nothing lands outside");
    }

    #[test]
    fn a_confined_write_replaces_an_existing_skill_file() {
        let home = TempDir::new().unwrap();
        let root = home.path().join(".codemux/skills");
        let dest = root.join("demo/SKILL.md");
        write_skill_to_disk(&dest, "first", Some(&root)).unwrap();
        write_skill_to_disk(&dest, "second", Some(&root)).unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "second");
        assert!(!dest.with_extension("md.tmp").exists());
    }

    #[test]
    fn a_confined_write_refuses_a_path_that_is_not_a_skill_file_under_its_root() {
        let home = TempDir::new().unwrap();
        let root = home.path().join(".codemux/skills");
        for dest in [root.join("demo/other.md"), root.join("a/b/SKILL.md"), root.join("SKILL.md")] {
            assert!(write_skill_to_disk(&dest, "x", Some(&root)).is_err(), "{}", dest.display());
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_temporary_file_is_never_followed() {
        let home = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let target = outside.path().join("victim");
        fs::write(&target, "original").unwrap();
        let dest = destination_path_after_pull("demo", home.path());
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, dest.with_extension("md.tmp")).unwrap();

        write_skill_to_disk(&dest, "new body", None).unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "new body");
        assert_eq!(fs::read_to_string(&target).unwrap(), "original");
    }

    // Unix only: Windows cannot create a folder named `a:b` at all; the
    // shared name rule's colon case is covered by the path tests.
    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial]
    async fn a_name_other_devices_cannot_use_is_not_uploaded() {
        // Any request would reach this server, which expects none.
        let mut server = mockito::Server::new_async().await;
        let no_calls = server.mock("POST", "/api/skills").expect(0).create_async().await;
        let prev_url = std::env::var("CODEMUX_API_URL").ok();
        std::env::set_var("CODEMUX_API_URL", server.url());
        let home = TempDir::new().unwrap();
        let engine = SyncEngine::with_home(home.path());
        let local = home.path().join(".codemux/skills/a:b/SKILL.md");
        fs::create_dir_all(local.parent().unwrap()).unwrap();
        fs::write(&local, "body").unwrap();

        let outcome = engine
            .push_one(&local, &mut SkillMapping::default(), "no-token", home.path())
            .await;
        match prev_url {
            Some(v) => std::env::set_var("CODEMUX_API_URL", v),
            None => std::env::remove_var("CODEMUX_API_URL"),
        }
        assert!(matches!(outcome, Ok(PushOutcome::Skipped)), "{outcome:?}");
        no_calls.assert_async().await;
    }

    #[test]
    fn apply_remote_skill_writes_when_server_newer_than_recorded() {
        let home = TempDir::new().unwrap();
        let engine = SyncEngine::with_home(home.path());

        // Seed an existing mapping pointing at a real on-disk
        // file with old content.
        let dest = destination_path_after_pull("demo", home.path());
        write_skill_to_disk(&dest, "old body", None).unwrap();
        let mut mapping = SkillMapping::default();
        mapping.upsert(MappingEntry {
            local_path: dest.clone(),
            remote_id: "1".into(),
            name: "demo".into(),
            provider: "codemux".into(),
            scope: "user".into(),
            last_synced_at_millis: 0,
            server_updated_at_millis: parse_iso_to_millis("2026-04-29T19:00:00Z").unwrap(),
        });

        // Server has newer content.
        let wire = plain_wire(
            "demo",
            "new body from server",
            "codemux",
            "user",
            "1",
            "2026-04-29T20:00:00Z",
        );

        let applied = engine
            .apply_remote_skill(
                &wire,
                &mut mapping,
                &mut LocalSkills::default(),
                home.path(),
                &home.path().join(".codemux/skills"),
            )
            .unwrap();
        assert!(applied);
        assert_eq!(fs::read_to_string(&dest).unwrap(), "new body from server");
    }

    #[test]
    fn apply_remote_skill_skips_legacy_row_with_empty_name() {
        // A ciphertext-only row that predates the server-side
        // migration comes back with an empty name. The engine must
        // skip it rather than writing an empty-named skill to disk.
        let home = TempDir::new().unwrap();
        let engine = SyncEngine::with_home(home.path());
        let mut mapping = SkillMapping::default();

        let wire = plain_wire("", "", "codemux", "user", "1", "2026-04-29T20:00:00Z");

        let applied = engine
            .apply_remote_skill(
                &wire,
                &mut mapping,
                &mut LocalSkills::default(),
                home.path(),
                &home.path().join(".codemux/skills"),
            )
            .unwrap();
        assert!(!applied, "legacy empty-name row must be skipped");
        assert!(mapping.skills.is_empty());
    }

    // ── snapshot / state transitions ───────────────────────────

    fn write_local_skill(home: &Path, root: &str, name: &str, content: &str) -> PathBuf {
        let path = home.join(root).join(name).join("SKILL.md");
        write_skill_to_disk(&path, content, None).unwrap();
        path
    }

    fn pull(engine: &SyncEngine, home: &Path, local_files: &[PathBuf], wires: &[SkillWire]) -> SkillMapping {
        let mut mapping = SkillMapping::default();
        let mut local = LocalSkills::new(local_files, &mapping, home);
        for wire in wires {
            engine
                .apply_remote_skill(wire, &mut mapping, &mut local, home, &home.join(".codemux/skills"))
                .unwrap();
        }
        mapping
    }

    #[test]
    fn unmapped_rows_link_to_the_local_skill_instead_of_duplicating_it() {
        let home = TempDir::new().unwrap();
        let engine = SyncEngine::with_home(home.path());
        let same = write_local_skill(home.path(), ".claude/skills", "same", "body");
        let edited = write_local_skill(home.path(), ".claude/skills", "edited", "local edit");
        let mapping = pull(&engine, home.path(), &[same.clone(), edited.clone()], &[
            plain_wire("same", "body", "claude", "user", "1", "2026-04-29T20:00:00Z"),
            plain_wire("edited", "older", "claude", "user", "2", "2026-04-29T20:00:00Z"),
            // Duplicate rows of a skill this device already owns.
            plain_wire("same", "body", "claude", "user", "3", "2026-04-29T20:00:00Z"),
            plain_wire("edited", "older", "claude", "user", "4", "2026-04-29T20:00:00Z"),
        ]);

        let linked: Vec<_> = mapping.skills.iter().map(|e| (e.remote_id.as_str(), e.local_path.clone())).collect();
        assert_eq!(linked, vec![("1", same.clone()), ("2", edited.clone())]);
        // Matching content is in sync; a differing file is pushed over the row.
        assert_eq!(mapping.skills[0].last_synced_at_millis, file_mtime_millis(&same).unwrap());
        assert_eq!(mapping.skills[1].last_synced_at_millis, 0);
        assert_eq!(fs::read_to_string(&edited).unwrap(), "local edit");
        assert!(!home.path().join(".codemux/skills").exists());
    }

    #[test]
    fn duplicate_rows_share_one_pulled_copy() {
        let home = TempDir::new().unwrap();
        let engine = SyncEngine::with_home(home.path());
        let mapping = pull(&engine, home.path(), &[], &[
            plain_wire("remote", "first", "claude", "user", "1", "2026-04-29T20:00:00Z"),
            plain_wire("remote", "second", "claude", "user", "2", "2026-04-29T20:00:00Z"),
        ]);
        let dest = destination_path_after_pull("remote", home.path());
        assert_eq!(mapping.skills.len(), 1);
        assert_eq!(mapping.skills[0].local_path, dest);
        assert_eq!(fs::read_to_string(dest).unwrap(), "first");
    }

    #[test]
    fn a_previously_pulled_copy_is_relinked_not_overwritten() {
        let home = TempDir::new().unwrap();
        let engine = SyncEngine::with_home(home.path());
        let copy = write_local_skill(home.path(), ".codemux/skills", "remote", "kept edit");
        let mapping = pull(&engine, home.path(), &[copy.clone()], &[
            plain_wire("remote", "server", "claude", "user", "1", "2026-04-29T20:00:00Z"),
        ]);
        assert_eq!(mapping.skills[0].local_path, copy);
        assert_eq!(fs::read_to_string(copy).unwrap(), "kept edit");
    }

    #[test]
    fn each_account_has_its_own_mapping_file() {
        let home = Path::new("/home/u");
        assert_ne!(account_mapping_path(home, "dev"), account_mapping_path(home, "main"));
        assert_eq!(
            account_mapping_path(home, "../a/b"),
            home.join(".codemux/sync/skills-mapping-ab.json"),
        );
    }

    #[test]
    fn fresh_engine_reports_idle_with_no_last_sync() {
        let home = TempDir::new().unwrap();
        let engine = SyncEngine::with_home(home.path());
        match engine.snapshot() {
            SyncStateSnapshot::Idle { last_sync_at_millis } => {
                assert!(last_sync_at_millis.is_none());
            }
            other => panic!("expected Idle, got {other:?}"),
        }
    }

    // ── write_skill_to_disk atomicity ──────────────────────────

    #[test]
    fn write_skill_creates_parent_dirs_and_consumes_tmp() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("nested/deeper/SKILL.md");
        write_skill_to_disk(&dest, "hello", None).unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "hello");
        assert!(!dest.with_extension("md.tmp").exists());
    }
}
