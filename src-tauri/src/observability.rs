use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredLogEntry {
    pub entry_id: String,
    pub source: String,
    pub level: LogLevel,
    pub message: String,
    pub metadata: Vec<(String, String)>,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub startup_count: u64,
    pub pane_count: u64,
    pub browser_operation_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureFlags {
    pub unstable_browser_automation: bool,
    pub unstable_indexing: bool,
    /// Gates the agent-chat pane kind, its Tauri command surface, and
    /// the runtime provider registry.
    ///
    /// Defaults to `true` — the Agent Chat GUI is the default
    /// interface (promoted out of Beta). When `false` (Settings →
    /// Interface toggle) the chat pane is not selectable in the UI,
    /// lifecycle commands return `FeatureDisabled`, and the provider
    /// registry is not initialised at startup — the classic
    /// terminal-first (CLI) interface renders instead.
    #[serde(default = "default_true")]
    pub enable_agent_chat: bool,
    /// Gates the lazy-workspace-creation path: sidebar-plus and
    /// boot-into-Home open a client-side chat draft instead of
    /// eagerly materialising a workspace. The draft is promoted to a
    /// real workspace on first message send.
    ///
    /// Defaults to `true`, paired with [`enable_agent_chat`] — the
    /// two always move together through `set_agent_chat_enabled`
    /// (every production read site pairs them with `&&`).
    ///
    /// [`enable_agent_chat`]: FeatureFlags::enable_agent_chat
    #[serde(default = "default_true")]
    pub enable_lazy_workspace_creation: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservabilitySnapshot {
    #[serde(default)]
    pub logs: Vec<StructuredLogEntry>,
    #[serde(default = "default_metrics_snapshot")]
    pub metrics: MetricsSnapshot,
    #[serde(default = "default_feature_flags")]
    pub feature_flags: FeatureFlags,
    /// One-time migration marker for the Agent Chat GUI promotion
    /// (Beta → default-on). Because the whole snapshot is re-saved on
    /// every log/metric mutation, every pre-promotion install has an
    /// explicit `enable_agent_chat: false` on disk (the old default) —
    /// indistinguishable from a deliberate opt-out. On the first load
    /// where no marker is found, both agent-chat flags are forced on
    /// and the marker is stamped; from then on the Settings → Interface
    /// toggle is fully respected.
    ///
    /// Read-only back-compat surface: the authoritative marker is the
    /// standalone `agent_chat_promoted` sentinel file next to this
    /// snapshot (see `promote_agent_chat_default`), because an older
    /// binary re-serializes the snapshot without this field and would
    /// otherwise erase it on a downgrade → upgrade round trip. This
    /// field is still honoured on read for installs promoted before
    /// the sentinel existed.
    #[serde(default)]
    pub agent_chat_promoted: bool,
}

fn default_metrics_snapshot() -> MetricsSnapshot {
    default_snapshot().metrics
}

fn default_feature_flags() -> FeatureFlags {
    default_snapshot().feature_flags
}

pub struct ObservabilityStore {
    inner: Arc<Mutex<ObservabilitySnapshot>>,
}

impl Default for ObservabilityStore {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(default_snapshot())),
        }
    }
}

impl ObservabilityStore {
    pub fn snapshot(&self) -> ObservabilitySnapshot {
        self.inner.lock().unwrap().clone()
    }

    pub fn log(
        &self,
        source: &str,
        level: LogLevel,
        message: String,
        metadata: Vec<(String, String)>,
    ) {
        let mut snapshot = self.inner.lock().unwrap();
        let entry_id = format!("log-{}", snapshot.logs.len() + 1);
        snapshot.logs.push(StructuredLogEntry {
            entry_id,
            source: source.into(),
            level,
            message,
            metadata,
            created_at_ms: current_time_ms(),
        });
        trim_logs(&mut snapshot.logs, 300);
        let _ = save_snapshot(&snapshot);
    }

    pub fn increment_metric(&self, key: &str) {
        let mut snapshot = self.inner.lock().unwrap();
        match key {
            "startup_count" => snapshot.metrics.startup_count += 1,
            "pane_count" => snapshot.metrics.pane_count += 1,
            "browser_operation_count" => snapshot.metrics.browser_operation_count += 1,
            _ => {}
        }
        let _ = save_snapshot(&snapshot);
    }

    /// Read the current feature-flag snapshot.
    ///
    /// Callers that only need one flag should use the explicit
    /// accessor (e.g. [`agent_chat_enabled`](Self::agent_chat_enabled))
    /// so the call site documents which flag it depends on.
    pub fn feature_flags(&self) -> FeatureFlags {
        self.inner.lock().unwrap().feature_flags.clone()
    }

    /// Whether the agent-chat pane and provider registry are enabled.
    ///
    /// This is a plain boolean read, cheap to call from command
    /// entry-points. The gate defaults to `false`; see
    /// [`FeatureFlags::enable_agent_chat`] for how to flip it.
    pub fn agent_chat_enabled(&self) -> bool {
        self.inner.lock().unwrap().feature_flags.enable_agent_chat
    }

    pub fn set_feature_flags(&self, flags: FeatureFlags) {
        let mut snapshot = self.inner.lock().unwrap();
        snapshot.feature_flags = flags;
        let _ = save_snapshot(&snapshot);
    }

}

pub fn load_observability_store() -> ObservabilityStore {
    let snapshot = load_or_migrate_snapshot(&snapshot_path(), &legacy_snapshot_path());
    ObservabilityStore {
        inner: Arc::new(Mutex::new(snapshot)),
    }
}

/// Outcome of trying to read a snapshot file, distinguishing "file is
/// absent" (safe to look elsewhere) from "file exists but is broken"
/// (fall back to defaults loudly — do NOT resurrect state from another
/// location, which could silently flip feature flags).
enum ReadOutcome {
    Loaded(Box<ObservabilitySnapshot>),
    Missing,
    Unreadable,
}

fn read_snapshot(path: &Path) -> ReadOutcome {
    match fs::read_to_string(path) {
        Ok(contents) => match serde_json::from_str::<ObservabilitySnapshot>(&contents) {
            Ok(snapshot) => ReadOutcome::Loaded(Box::new(snapshot)),
            Err(error) => {
                // Don't swallow: partial/corrupt JSON silently defaulting to
                // `enable_agent_chat: false` cost us hours. Log and continue.
                eprintln!(
                    "[codemux::observability] Failed to parse {}: {error}. Falling back to defaults.",
                    path.display()
                );
                ReadOutcome::Unreadable
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ReadOutcome::Missing,
        Err(error) => {
            eprintln!(
                "[codemux::observability] Failed to read {}: {error}. Falling back to defaults.",
                path.display()
            );
            ReadOutcome::Unreadable
        }
    }
}

/// Load the snapshot from the per-build `path`; when it doesn't exist
/// yet, fall back to the legacy machine-shared location and copy it
/// forward once.
///
/// The copy (rather than a move) keeps the legacy file intact so an
/// older installed release that still reads `~/.codemux/` keeps
/// working after a downgrade. After migration each build owns its own
/// file, so they diverge from that point on — which is the fix for
/// the shared-flags bug (see [`snapshot_path`]).
fn load_or_migrate_snapshot(path: &Path, legacy_path: &Path) -> ObservabilitySnapshot {
    let mut snapshot = match read_snapshot(path) {
        ReadOutcome::Loaded(snapshot) => *snapshot,
        ReadOutcome::Unreadable => default_snapshot(),
        ReadOutcome::Missing => match read_snapshot(legacy_path) {
            ReadOutcome::Loaded(snapshot) => {
                if let Err(error) = save_snapshot_to(path, &snapshot) {
                    // Non-fatal: the next mutation re-attempts the write via
                    // `save_snapshot`. Until then we serve the legacy state.
                    eprintln!(
                        "[codemux::observability] Failed to migrate legacy snapshot {} -> {}: {error}",
                        legacy_path.display(),
                        path.display()
                    );
                }
                *snapshot
            }
            _ => default_snapshot(),
        },
    };
    promote_agent_chat_default(&mut snapshot, path);
    snapshot
}

/// One-time promotion of the Agent Chat GUI from opt-in Beta to the
/// default interface.
///
/// Pre-promotion snapshots carry an explicit `enable_agent_chat:
/// false` written by the old default (the full snapshot is persisted
/// on every mutation), so flipping the serde/`default_snapshot`
/// literals alone would leave every existing install in CLI mode. The
/// first load with no promotion marker forces both paired flags on and
/// stamps the marker; any later opt-out via Settings → Interface is
/// never overridden again.
///
/// The marker lives in a standalone sentinel file next to the snapshot
/// ([`promotion_marker_path`]), *not* only in the snapshot JSON. An
/// older binary that doesn't know the
/// [`ObservabilitySnapshot::agent_chat_promoted`] field drops it on
/// its next write (serde ignores unknown keys on deserialize and
/// re-serializes without them), so a downgrade → upgrade round trip
/// would erase a snapshot-only marker, re-run the promotion, and
/// force-revert a deliberate opt-out. Old binaries never touch the
/// sentinel file, so it survives. The snapshot field is still honoured
/// on read for back-compat with installs promoted before this change.
fn promote_agent_chat_default(snapshot: &mut ObservabilitySnapshot, path: &Path) {
    let marker_path = promotion_marker_path(path);
    let marker_file_present = marker_path.exists();
    // Either marker counts: the sentinel file (written from here on)
    // or the legacy snapshot field (installs promoted before the
    // sentinel existed, and fresh installs, whose `default_snapshot`
    // is already the promoted state).
    let already_promoted = marker_file_present || snapshot.agent_chat_promoted;

    if !already_promoted {
        snapshot.feature_flags.enable_agent_chat = true;
        snapshot.feature_flags.enable_lazy_workspace_creation = true;
    }
    snapshot.agent_chat_promoted = true;

    if !marker_file_present {
        // Written on every path, including the "already promoted via
        // the snapshot field" and fresh-install ones — that is what
        // makes a later downgrade → upgrade cycle safe. Cheap: one
        // tiny write, once per install.
        if let Err(error) = write_promotion_marker(&marker_path) {
            // Non-fatal: without the sentinel we simply fall back to
            // the snapshot field, i.e. today's behavior.
            eprintln!("[codemux::observability] {error}");
        }
    }

    if !already_promoted {
        if let Err(error) = save_snapshot_to(path, snapshot) {
            // Non-fatal: the promotion re-applies on every boot until a
            // save sticks, and any snapshot mutation persists it too.
            eprintln!(
                "[codemux::observability] Failed to persist agent-chat promotion to {}: {error}",
                path.display()
            );
        }
    }
}

/// Sentinel file recording that the one-time Agent Chat GUI promotion
/// has already been applied — `<data root>/agent_chat_promoted`,
/// alongside `observability.json`. A standalone file (rather than only
/// a snapshot key) because older binaries rewrite the snapshot without
/// keys they don't know about, but never delete unrelated files.
fn promotion_marker_path(snapshot_path: &Path) -> PathBuf {
    snapshot_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("agent_chat_promoted")
}

fn write_promotion_marker(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create observability dir: {error}"))?;
    }
    fs::write(
        path,
        "Agent Chat GUI promotion applied. Delete to re-run the one-time upgrade.\n",
    )
    .map_err(|error| {
        format!(
            "Failed to write agent-chat promotion marker {}: {error}",
            path.display()
        )
    })
}

fn default_snapshot() -> ObservabilitySnapshot {
    ObservabilitySnapshot {
        logs: vec![],
        metrics: MetricsSnapshot {
            startup_count: 0,
            pane_count: 0,
            browser_operation_count: 0,
        },
        feature_flags: FeatureFlags {
            unstable_browser_automation: true,
            unstable_indexing: true,
            // The Agent Chat GUI is the default interface (promoted out
            // of Beta). The `Settings → Interface` toggle flips both
            // flags atomically back to the classic CLI view — they're
            // paired in every production read site, so the user-facing
            // decision is one switch. Existing installs are upgraded
            // once via `promote_agent_chat_default`; an opt-out made
            // after that is respected (the persisted file wins over
            // these literals).
            enable_agent_chat: true,
            enable_lazy_workspace_creation: true,
        },
        // Fresh installs never need the one-time promotion — the flag
        // literals above are already the promoted state.
        agent_chat_promoted: true,
    }
}

fn save_snapshot(snapshot: &ObservabilitySnapshot) -> Result<(), String> {
    save_snapshot_to(&snapshot_path(), snapshot)
}

fn save_snapshot_to(path: &Path, snapshot: &ObservabilitySnapshot) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create observability dir: {error}"))?;
    }

    let json = serde_json::to_string_pretty(snapshot)
        .map_err(|error| format!("Failed to serialize observability snapshot: {error}"))?;
    fs::write(path, json).map_err(|error| {
        format!(
            "Failed to write observability snapshot {}: {error}",
            path.display()
        )
    })
}

/// Per-build snapshot location:
/// `~/.local/share/codemux/observability.json` for release builds,
/// `~/.local/share/codemux-dev/observability.json` for debug builds
/// (platform-specific `dirs::data_dir()` on Windows/macOS).
///
/// Two hard-won constraints:
///
/// 1. Anchor to the XDG data dir, not CWD. CWD drifts between build
///    modes (`cargo tauri dev` launches from `src-tauri/`, the
///    installed binary from wherever the user invoked it), so a
///    CWD-relative path produces a different file per launch —
///    feature flags appear to "not stick" across
///    restarts. See feature/agent-chat debugging for the incident.
/// 2. Scope by `APP_DIR_NAME`, like every other piece of local state
///    (sqlite db, auth tokens, settings cache). The previous location,
///    `~/.codemux/observability.json`, was shared by every Codemux
///    build on the machine, so flipping Settings → Interface → Agent
///    Chat GUI in a dev build also flipped it in the installed
///    release (and vice versa) — feature flags leaked across
///    otherwise-isolated instances and accounts.
fn snapshot_path() -> PathBuf {
    data_root().join("observability.json")
}

/// Mirrors `settings_sync::cache_dir()` — the canonical per-build
/// local-state root.
fn data_root() -> PathBuf {
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/share"));
    data_dir.join(crate::APP_DIR_NAME)
}

/// Pre-migration location, shared by every build on the machine.
/// Read-only from here on: consulted once when the per-build file
/// doesn't exist yet, never written to. Left in place so older
/// installed versions that still read it keep working.
fn legacy_snapshot_path() -> PathBuf {
    let root = dirs::home_dir()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    root.join(".codemux").join("observability.json")
}

fn trim_logs(logs: &mut Vec<StructuredLogEntry>, max: usize) {
    if logs.len() > max {
        let remove_count = logs.len() - max;
        logs.drain(0..remove_count);
    }
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Regresses two path bugs:
    //
    // 1. CWD drift: an early CWD-relative `snapshot_path` resolved to
    //    `src-tauri/.codemux/observability.json` under `cargo tauri
    //    dev` and a different file under the installed binary — flags
    //    appeared to "not stick". The path must not depend on CWD.
    // 2. Cross-build leak: the follow-up `$HOME/.codemux/` anchor was
    //    shared by every build on the machine, so the Agent Chat GUI
    //    toggle flipped in BOTH the installed release and a dev build
    //    at once. The path must be scoped by `APP_DIR_NAME` like the
    //    rest of local state.
    #[test]
    #[serial_test::serial]
    fn snapshot_path_is_build_scoped_and_cwd_independent() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let prev_cwd = std::env::current_dir().expect("current_dir");

        let path_before = snapshot_path();
        std::env::set_current_dir(tmp.path()).expect("set_current_dir to tempdir");
        let path_after = snapshot_path();

        std::env::set_current_dir(&prev_cwd).expect("restore cwd");

        assert_eq!(
            path_before, path_after,
            "snapshot_path() must not depend on CWD"
        );
        assert!(
            path_after.ends_with(
                Path::new(crate::APP_DIR_NAME).join("observability.json")
            ),
            "snapshot_path() = {} must be scoped by APP_DIR_NAME = {} so dev and release builds don't share feature flags",
            path_after.display(),
            crate::APP_DIR_NAME
        );
        assert!(
            !path_after
                .components()
                .any(|c| c.as_os_str() == ".codemux"),
            "snapshot_path() = {} must not resolve to the legacy machine-shared ~/.codemux location",
            path_after.display()
        );
    }

    /// First launch after the path change: the per-build file doesn't
    /// exist, but the legacy machine-shared one does. The legacy state
    /// must be adopted (users who opted into the Beta keep it) and
    /// copied into the per-build location so subsequent flips no
    /// longer touch shared state. The legacy file itself stays intact
    /// for older installed versions.
    #[test]
    fn load_migrates_legacy_snapshot_once() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let legacy = tmp.path().join(".codemux").join("observability.json");
        let new = tmp.path().join("data").join("observability.json");

        let mut snapshot = default_snapshot();
        snapshot.feature_flags.enable_agent_chat = true;
        snapshot.feature_flags.enable_lazy_workspace_creation = true;
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(&legacy, serde_json::to_string_pretty(&snapshot).unwrap()).unwrap();

        let loaded = load_or_migrate_snapshot(&new, &legacy);

        assert!(loaded.feature_flags.enable_agent_chat, "legacy opt-in must survive the path move");
        assert!(new.exists(), "legacy snapshot must be copied to the per-build path");
        assert!(legacy.exists(), "legacy file must be left in place for older versions");

        let copied: ObservabilitySnapshot =
            serde_json::from_str(&fs::read_to_string(&new).unwrap()).unwrap();
        assert!(copied.feature_flags.enable_agent_chat);
    }

    /// Once the per-build file exists it is authoritative — the legacy
    /// shared file must never override it again, otherwise the
    /// cross-build leak comes back through the migration path.
    #[test]
    fn per_build_snapshot_wins_over_legacy() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let legacy = tmp.path().join(".codemux").join("observability.json");
        let new = tmp.path().join("data").join("observability.json");

        let mut legacy_snapshot = default_snapshot();
        legacy_snapshot.feature_flags.enable_agent_chat = true;
        legacy_snapshot.feature_flags.enable_lazy_workspace_creation = true;
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(&legacy, serde_json::to_string_pretty(&legacy_snapshot).unwrap()).unwrap();

        // Per-build state: user explicitly opted out of the GUI after
        // the promotion already ran (marker stamped).
        let mut new_snapshot = default_snapshot();
        new_snapshot.feature_flags.enable_agent_chat = false;
        new_snapshot.feature_flags.enable_lazy_workspace_creation = false;
        fs::create_dir_all(new.parent().unwrap()).unwrap();
        fs::write(&new, serde_json::to_string_pretty(&new_snapshot).unwrap()).unwrap();

        let loaded = load_or_migrate_snapshot(&new, &legacy);

        assert!(
            !loaded.feature_flags.enable_agent_chat,
            "the per-build snapshot must win over the legacy shared file"
        );
    }

    /// Fresh machine: neither file exists. Defaults apply and nothing
    /// is written until the first real mutation.
    #[test]
    fn load_defaults_when_no_snapshot_exists() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let legacy = tmp.path().join(".codemux").join("observability.json");
        let new = tmp.path().join("data").join("observability.json");

        let loaded = load_or_migrate_snapshot(&new, &legacy);

        assert!(
            loaded.feature_flags.enable_agent_chat,
            "fresh install must boot into the Agent Chat GUI"
        );
        assert!(!new.exists(), "load must not create files when there is nothing to migrate");
    }

    /// The Agent Chat GUI is the default interface for new users. The
    /// `default_snapshot()` literals and the serde defaults for the
    /// flag fields must agree, otherwise a fresh-install user and a
    /// partial-config user see different interfaces.
    #[test]
    fn default_snapshot_enables_agent_chat_gui() {
        let snap = default_snapshot();
        assert!(
            snap.feature_flags.enable_agent_chat,
            "default_snapshot must have enable_agent_chat = true; the GUI is the default interface"
        );
        assert!(
            snap.feature_flags.enable_lazy_workspace_creation,
            "default_snapshot must have enable_lazy_workspace_creation = true; paired with enable_agent_chat as one toggle"
        );
        assert!(
            snap.agent_chat_promoted,
            "fresh installs must not re-run the one-time promotion"
        );
    }

    /// The `default_true` serde default on the flag fields must
    /// produce the same on-default as `default_snapshot()`. Drift
    /// between the two produces "fresh install ON, partial-config
    /// user OFF" surprises.
    #[test]
    fn serde_default_for_agent_chat_flags_is_true() {
        // FeatureFlags requires the unstable_* fields, but the agent-
        // chat fields have serde defaults. A partial JSON missing
        // those fields must default them to true, matching
        // `default_snapshot()`'s on-state.
        let json = r#"{
            "unstable_browser_automation": true,
            "unstable_indexing": true
        }"#;
        let parsed: FeatureFlags = serde_json::from_str(json)
            .expect("FeatureFlags missing only the agent-chat keys parses with serde defaults");
        assert!(
            parsed.enable_agent_chat,
            "serde default for enable_agent_chat must be true to match default_snapshot()"
        );
        assert!(
            parsed.enable_lazy_workspace_creation,
            "serde default for enable_lazy_workspace_creation must be true to match default_snapshot()"
        );
    }

    /// An explicit opt-out persisted in observability.json must win
    /// over the on-default — serde only fills missing fields, it never
    /// overrides written values.
    #[test]
    fn persisted_agent_chat_false_survives_default_flip() {
        let json = r#"{
            "unstable_browser_automation": true,
            "unstable_indexing": true,
            "enable_agent_chat": false,
            "enable_lazy_workspace_creation": false
        }"#;
        let parsed: FeatureFlags = serde_json::from_str(json).expect("full flags parse");
        assert!(!parsed.enable_agent_chat);
        assert!(!parsed.enable_lazy_workspace_creation);
    }

    /// Write a snapshot to `path` with the `agent_chat_promoted` key
    /// stripped — exactly what an older binary leaves behind, since
    /// serde drops unknown keys on deserialize and re-serializes
    /// without them.
    fn write_snapshot_without_marker_field(path: &Path, snapshot: &ObservabilitySnapshot) {
        let mut value = serde_json::to_value(snapshot).unwrap();
        value.as_object_mut().unwrap().remove("agent_chat_promoted");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    }

    /// A pre-promotion snapshot (no marker anywhere) with the flags
    /// explicitly `false` — i.e. every install that simply never
    /// touched the old Beta toggle — must be upgraded to the GUI
    /// exactly once, with the marker persisted so the upgrade never
    /// re-runs.
    #[test]
    fn promotion_upgrades_pre_promotion_snapshot_once() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let legacy = tmp.path().join(".codemux").join("observability.json");
        let new = tmp.path().join("data").join("observability.json");

        // Hand-write a pre-promotion file: flags explicitly false, no
        // marker key at all (serde default = false).
        let mut old = default_snapshot();
        old.feature_flags.enable_agent_chat = false;
        old.feature_flags.enable_lazy_workspace_creation = false;
        write_snapshot_without_marker_field(&new, &old);

        let loaded = load_or_migrate_snapshot(&new, &legacy);

        assert!(loaded.feature_flags.enable_agent_chat, "promotion must flip the GUI on");
        assert!(loaded.feature_flags.enable_lazy_workspace_creation);
        assert!(loaded.agent_chat_promoted, "promotion must stamp the marker");

        // The upgraded state must be persisted so the next boot
        // doesn't depend on re-running the migration.
        let persisted: ObservabilitySnapshot =
            serde_json::from_str(&fs::read_to_string(&new).unwrap()).unwrap();
        assert!(persisted.feature_flags.enable_agent_chat);
        assert!(persisted.agent_chat_promoted);

        // …and the downgrade-proof sentinel is written alongside it.
        assert!(
            promotion_marker_path(&new).exists(),
            "promotion must drop the standalone sentinel file"
        );
    }

    /// After the promotion has run, a user's explicit opt-out (flags
    /// false, marker true) is respected on every subsequent load.
    #[test]
    fn promotion_respects_post_promotion_opt_out() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let legacy = tmp.path().join(".codemux").join("observability.json");
        let new = tmp.path().join("data").join("observability.json");

        let mut opted_out = default_snapshot();
        opted_out.feature_flags.enable_agent_chat = false;
        opted_out.feature_flags.enable_lazy_workspace_creation = false;
        assert!(opted_out.agent_chat_promoted, "default_snapshot carries the marker");
        fs::create_dir_all(new.parent().unwrap()).unwrap();
        fs::write(&new, serde_json::to_string_pretty(&opted_out).unwrap()).unwrap();

        let loaded = load_or_migrate_snapshot(&new, &legacy);

        assert!(
            !loaded.feature_flags.enable_agent_chat,
            "an opt-out made after the promotion must never be overridden"
        );
        assert!(!loaded.feature_flags.enable_lazy_workspace_creation);
        // Reading a legacy field-only marker must upgrade it to the
        // sentinel file, otherwise the next downgrade erases it.
        assert!(
            promotion_marker_path(&new).exists(),
            "a field-only marker must be mirrored into the sentinel file"
        );
    }

    /// The downgrade → upgrade round trip. An older binary rewrites
    /// `observability.json` without the `agent_chat_promoted` key
    /// (serde ignores unknown fields), so a snapshot-only marker
    /// vanishes. If that were the only marker, the promotion would
    /// re-run and force-revert a deliberate opt-out. The standalone
    /// sentinel file — which old binaries never touch — must keep the
    /// opt-out intact.
    #[test]
    fn opt_out_survives_marker_field_erasure_via_sentinel() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let legacy = tmp.path().join(".codemux").join("observability.json");
        let new = tmp.path().join("data").join("observability.json");

        // User opted out on a current binary…
        let mut opted_out = default_snapshot();
        opted_out.feature_flags.enable_agent_chat = false;
        opted_out.feature_flags.enable_lazy_workspace_creation = false;
        // …then downgraded: the old binary rewrote the file, dropping
        // the marker field but leaving the sentinel alone.
        write_snapshot_without_marker_field(&new, &opted_out);
        write_promotion_marker(&promotion_marker_path(&new)).expect("write sentinel");

        let loaded = load_or_migrate_snapshot(&new, &legacy);

        assert!(
            !loaded.feature_flags.enable_agent_chat,
            "the sentinel must stop the promotion from re-running over an opt-out"
        );
        assert!(!loaded.feature_flags.enable_lazy_workspace_creation);
        assert!(loaded.agent_chat_promoted, "the sentinel re-stamps the in-memory marker");
    }

    /// Without the sentinel the same file *is* a genuine pre-promotion
    /// install and must be upgraded — pins that the previous test
    /// passes because of the sentinel, not because of the flags.
    #[test]
    fn marker_field_erasure_without_sentinel_still_promotes() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let legacy = tmp.path().join(".codemux").join("observability.json");
        let new = tmp.path().join("data").join("observability.json");

        let mut old = default_snapshot();
        old.feature_flags.enable_agent_chat = false;
        old.feature_flags.enable_lazy_workspace_creation = false;
        write_snapshot_without_marker_field(&new, &old);

        let loaded = load_or_migrate_snapshot(&new, &legacy);

        assert!(loaded.feature_flags.enable_agent_chat);
    }

    /// Fresh install: nothing to migrate, but the sentinel is still
    /// stamped so a later opt-out can't be undone by a downgrade →
    /// upgrade cycle. The snapshot itself stays unwritten until a real
    /// mutation.
    #[test]
    fn fresh_install_stamps_the_sentinel_without_writing_a_snapshot() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let legacy = tmp.path().join(".codemux").join("observability.json");
        let new = tmp.path().join("data").join("observability.json");

        let loaded = load_or_migrate_snapshot(&new, &legacy);

        assert!(loaded.feature_flags.enable_agent_chat);
        assert!(loaded.agent_chat_promoted);
        assert!(!new.exists(), "no snapshot is written when there is nothing to migrate");
        assert!(
            promotion_marker_path(&new).exists(),
            "a fresh install is already promoted — record it so a downgrade can't undo it"
        );
    }

    /// `set_agent_chat_enabled` must flip both fields together. The
    /// chat surface assumes both flags are true to function — leaving
    /// one of them flipped without the other lights up half the
    /// surface and breaks composer routing. Pin the contract at the
    /// store level so any future setter that bypasses the command
    /// still gets caught.
    #[test]
    fn agent_chat_flags_can_be_flipped_atomically_via_store() {
        let store = ObservabilityStore::default();
        let starting = store.feature_flags();
        assert!(starting.enable_agent_chat, "default is on (GUI is the default interface)");
        assert!(starting.enable_lazy_workspace_creation, "default is on");

        // Mirror what `commands::set_agent_chat_enabled(enabled=false)`
        // does — the Settings → Interface opt-out back to CLI.
        let mut next = store.feature_flags();
        next.enable_agent_chat = false;
        next.enable_lazy_workspace_creation = false;
        store.set_feature_flags(next);

        let after_off = store.feature_flags();
        assert!(!after_off.enable_agent_chat);
        assert!(!after_off.enable_lazy_workspace_creation);

        // And the symmetric on flip.
        let mut next = store.feature_flags();
        next.enable_agent_chat = true;
        next.enable_lazy_workspace_creation = true;
        store.set_feature_flags(next);

        let after_on = store.feature_flags();
        assert!(after_on.enable_agent_chat);
        assert!(after_on.enable_lazy_workspace_creation);

        // Other flags untouched by the toggle.
        assert!(after_on.unstable_browser_automation);
        assert!(after_on.unstable_indexing);
    }
}
