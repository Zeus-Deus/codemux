// Skills sync — local↔remote ID mapping.
//
// A SKILL.md on disk has no remote_id by itself. The push pipeline
// needs to know "is this a new skill (POST) or an update (PUT
// /api/skills/:id)?", and the pull pipeline needs to know
// "have I already written a local file for this remote_id, and
// where?". The mapping table answers both.
//
// Format: a single JSON file at `~/.codemux/sync/skills-mapping.json`
// with a versioned envelope. Single file keeps the inspect/repair
// story simple — `cat`, `jq`, delete-and-resync if it ever gets
// corrupted. Per the Stage 3 spec, the alternative SQLite table
// won every other ergonomic axis but lost on "user can hand-fix
// without launching a tool."
//
// Atomicity: writes go to a sibling `.tmp` file and are renamed
// over the canonical path. Rename is atomic on POSIX and
// near-atomic on Windows (the renamer's inode swap can be
// observed mid-flight only in narrow edge cases, none of which
// matter for a JSON config file). A crash during the write leaves
// either the old file intact or the new file intact, never a
// half-written one.
//
// Corruption recovery: a malformed file is logged and treated as
// "no mappings yet" so the pull pipeline rebuilds the table from
// the server's authoritative state. The on-disk garbage gets
// overwritten by the next successful flush. We never delete the
// corrupted file ourselves — preserving evidence helps debugging.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Schema version. Bump when the on-disk layout changes in a
/// non-additive way; readers below this version drop the file and
/// rebuild.
pub const MAPPING_SCHEMA_VERSION: u32 = 1;

/// Per-skill mapping row. `local_path` is canonical (we always
/// resolve symlinks before writing) so duplicate entries can't
/// sneak in via `~/.codemux/skills/foo` vs `/home/u/.codemux/...`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MappingEntry {
    pub local_path: PathBuf,
    /// Server-assigned BIGSERIAL id, serialized as a string on the
    /// wire (Stage 1 contract: bigint > JS Number.MAX_SAFE_INTEGER).
    pub remote_id: String,
    /// Skill name in plaintext. Cached locally because the server
    /// stores only the encrypted name; pulling without a cached
    /// name means decrypt-everything-to-find-this-row.
    pub name: String,
    pub provider: String,
    pub scope: String,
    /// Last successful sync timestamp as Unix millis. The push
    /// pipeline compares this against the file's mtime to suppress
    /// the "we just wrote this during pull" echo from the file
    /// watcher.
    pub last_synced_at_millis: u128,
    /// Server-side updated_at at the time of last sync, Unix
    /// millis. Conflict resolution compares this against the
    /// server's freshly-read updated_at to detect remote edits.
    pub server_updated_at_millis: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillMapping {
    pub version: u32,
    #[serde(default)]
    pub skills: Vec<MappingEntry>,
}

impl Default for SkillMapping {
    fn default() -> Self {
        Self {
            version: MAPPING_SCHEMA_VERSION,
            skills: Vec::new(),
        }
    }
}

impl SkillMapping {
    /// Find the entry for a local path, if any.
    pub fn find_by_path(&self, path: &Path) -> Option<&MappingEntry> {
        self.skills.iter().find(|e| e.local_path == path)
    }

    /// Find the entry for a server-assigned remote_id, if any.
    pub fn find_by_remote_id(&self, remote_id: &str) -> Option<&MappingEntry> {
        self.skills.iter().find(|e| e.remote_id == remote_id)
    }

    /// Replace or insert. Equality keyed on `remote_id` —
    /// `local_path` may legitimately change (the file got moved
    /// after a pull, the user renamed the directory) but
    /// `remote_id` is the immutable spine.
    pub fn upsert(&mut self, entry: MappingEntry) {
        if let Some(existing) = self
            .skills
            .iter_mut()
            .find(|e| e.remote_id == entry.remote_id)
        {
            *existing = entry;
        } else {
            self.skills.push(entry);
        }
    }

    /// Drop the entry for a given remote_id. No-op when absent.
    pub fn remove_by_remote_id(&mut self, remote_id: &str) {
        self.skills.retain(|e| e.remote_id != remote_id);
    }
}

// ────────────────────────────────────────────────────────────────
// Disk I/O.

/// Default mapping file location.
/// `~/.codemux/sync/skills-mapping.json` — separate from the
/// skills payload directory so a "rm -rf ~/.codemux/skills" wipe
/// doesn't take the mapping with it (the user might want to
/// re-pull on a fresh skills/ dir without re-creating every
/// skill).
pub fn default_mapping_path(home: &Path) -> PathBuf {
    home.join(".codemux/sync/skills-mapping.json")
}

/// Read the mapping file. Missing file → fresh empty mapping.
/// Malformed file → log + fresh empty mapping (the next flush
/// will overwrite the garbage; the original is renamed to
/// `.corrupted-<unix-millis>` for forensics).
pub fn load_mapping(path: &Path) -> SkillMapping {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return SkillMapping::default();
        }
        Err(err) => {
            eprintln!(
                "[skills_sync] mapping read failed at {}: {err} — starting fresh",
                path.display()
            );
            return SkillMapping::default();
        }
    };

    match serde_json::from_slice::<SkillMapping>(&bytes) {
        Ok(m) if m.version == MAPPING_SCHEMA_VERSION => m,
        Ok(other) => {
            eprintln!(
                "[skills_sync] mapping file at {} has unknown version {} — starting fresh",
                path.display(),
                other.version
            );
            preserve_corrupted(path);
            SkillMapping::default()
        }
        Err(err) => {
            eprintln!(
                "[skills_sync] mapping file at {} is corrupted ({err}) — starting fresh",
                path.display()
            );
            preserve_corrupted(path);
            SkillMapping::default()
        }
    }
}

/// Write the mapping file atomically. Creates parent dirs as
/// needed. Returns Err only on genuine I/O failures (filesystem
/// full, no permission); a serialization failure is unrecoverable
/// at the caller level and bubbles up as a string.
pub fn save_mapping(path: &Path, mapping: &SkillMapping) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create mapping dir: {e}"))?;
    }

    let json = serde_json::to_vec_pretty(mapping)
        .map_err(|e| format!("serialize mapping: {e}"))?;

    let tmp = path.with_extension("tmp");
    fs::write(&tmp, &json).map_err(|e| format!("write tmp mapping: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| format!("rename mapping: {e}"))?;
    Ok(())
}

/// On corruption: rename the bad file aside so a future debugger
/// can inspect it. Best-effort; rename failures are logged and
/// swallowed so the caller can still proceed with a fresh
/// mapping.
fn preserve_corrupted(path: &Path) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let preserved = path.with_extension(format!("corrupted-{stamp}"));
    if let Err(err) = fs::rename(path, &preserved) {
        eprintln!(
            "[skills_sync] could not rename corrupted mapping aside ({err}); leaving in place"
        );
    } else {
        eprintln!(
            "[skills_sync] preserved corrupted mapping at {}",
            preserved.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn entry(remote_id: &str, name: &str, local_path: &Path) -> MappingEntry {
        MappingEntry {
            local_path: local_path.to_path_buf(),
            remote_id: remote_id.into(),
            name: name.into(),
            provider: "codemux".into(),
            scope: "user".into(),
            last_synced_at_millis: 1_000_000,
            server_updated_at_millis: 999_999,
        }
    }

    // ── Default + lookup ───────────────────────────────────────

    #[test]
    fn default_mapping_is_versioned_empty() {
        let m = SkillMapping::default();
        assert_eq!(m.version, MAPPING_SCHEMA_VERSION);
        assert!(m.skills.is_empty());
    }

    #[test]
    fn find_by_path_returns_match() {
        let mut m = SkillMapping::default();
        let local = PathBuf::from("/home/u/.codemux/skills/foo/SKILL.md");
        m.upsert(entry("1", "foo", &local));
        assert!(m.find_by_path(&local).is_some());
    }

    #[test]
    fn find_by_path_returns_none_for_unknown() {
        let m = SkillMapping::default();
        assert!(
            m.find_by_path(&PathBuf::from("/nope")).is_none()
        );
    }

    #[test]
    fn find_by_remote_id_returns_match() {
        let mut m = SkillMapping::default();
        m.upsert(entry("42", "x", &PathBuf::from("/p")));
        assert_eq!(m.find_by_remote_id("42").unwrap().name, "x");
        assert!(m.find_by_remote_id("99").is_none());
    }

    // ── Upsert semantics ──────────────────────────────────────

    #[test]
    fn upsert_replaces_existing_by_remote_id() {
        let mut m = SkillMapping::default();
        let original = PathBuf::from("/home/u/.codemux/skills/foo/SKILL.md");
        let renamed = PathBuf::from("/home/u/.codemux/skills/foo-renamed/SKILL.md");

        m.upsert(entry("7", "foo", &original));
        let mut updated = entry("7", "foo-renamed", &renamed);
        updated.last_synced_at_millis = 2_000_000;
        m.upsert(updated.clone());

        assert_eq!(m.skills.len(), 1, "upsert should replace, not duplicate");
        let row = m.find_by_remote_id("7").unwrap();
        assert_eq!(row.local_path, renamed);
        assert_eq!(row.last_synced_at_millis, 2_000_000);
    }

    #[test]
    fn upsert_inserts_when_remote_id_is_new() {
        let mut m = SkillMapping::default();
        m.upsert(entry("1", "a", &PathBuf::from("/p1")));
        m.upsert(entry("2", "b", &PathBuf::from("/p2")));
        assert_eq!(m.skills.len(), 2);
    }

    #[test]
    fn remove_by_remote_id_drops_the_row() {
        let mut m = SkillMapping::default();
        m.upsert(entry("1", "a", &PathBuf::from("/p1")));
        m.upsert(entry("2", "b", &PathBuf::from("/p2")));
        m.remove_by_remote_id("1");
        assert_eq!(m.skills.len(), 1);
        assert_eq!(m.skills[0].remote_id, "2");
    }

    #[test]
    fn remove_by_remote_id_is_noop_for_missing() {
        let mut m = SkillMapping::default();
        m.upsert(entry("1", "a", &PathBuf::from("/p1")));
        m.remove_by_remote_id("999");
        assert_eq!(m.skills.len(), 1);
    }

    // ── Disk I/O ──────────────────────────────────────────────

    #[test]
    fn load_missing_file_returns_default() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("missing.json");
        let m = load_mapping(&p);
        assert_eq!(m, SkillMapping::default());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("m.json");
        let mut original = SkillMapping::default();
        original.upsert(entry("42", "demo", &PathBuf::from("/p")));
        save_mapping(&p, &original).unwrap();

        let recovered = load_mapping(&p);
        assert_eq!(recovered, original);
    }

    #[test]
    fn save_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("nested/sub/dirs/m.json");
        let mut m = SkillMapping::default();
        m.upsert(entry("1", "x", &PathBuf::from("/p")));
        save_mapping(&p, &m).unwrap();
        assert!(p.exists());
    }

    #[test]
    fn corrupted_json_treats_as_empty_and_preserves_evidence() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("m.json");
        fs::write(&p, b"{this is not json").unwrap();

        let m = load_mapping(&p);
        assert_eq!(m, SkillMapping::default());

        // Corruption preserved aside.
        let preserved: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .contains("corrupted-")
            })
            .collect();
        assert_eq!(preserved.len(), 1);
        // Original path is now empty (renamed away).
        assert!(!p.exists());
    }

    #[test]
    fn unknown_version_treats_as_empty() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("m.json");
        fs::write(
            &p,
            br#"{"version":999,"skills":[{"local_path":"/p","remote_id":"1","name":"x","provider":"codemux","scope":"user","last_synced_at_millis":0,"server_updated_at_millis":0}]}"#,
        )
        .unwrap();

        let m = load_mapping(&p);
        assert!(m.skills.is_empty());
        assert_eq!(m.version, MAPPING_SCHEMA_VERSION);
    }

    #[test]
    fn save_overwrites_atomically_via_tmp_rename() {
        // Sanity: after a successful save, the .tmp sibling must be
        // gone (rename consumes it). If the rename ever stops
        // working, we'd see leftover .tmp files in production —
        // catch that here.
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("m.json");
        let mut m = SkillMapping::default();
        m.upsert(entry("1", "x", &PathBuf::from("/p")));
        save_mapping(&p, &m).unwrap();

        let tmp = p.with_extension("tmp");
        assert!(!tmp.exists(), "tmp should be consumed by rename");
        assert!(p.exists());
    }

    #[test]
    fn save_multiple_times_retains_latest_only() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("m.json");

        let mut m = SkillMapping::default();
        m.upsert(entry("1", "first", &PathBuf::from("/p1")));
        save_mapping(&p, &m).unwrap();

        m.upsert(entry("2", "second", &PathBuf::from("/p2")));
        save_mapping(&p, &m).unwrap();

        let recovered = load_mapping(&p);
        assert_eq!(recovered.skills.len(), 2);
    }
}
