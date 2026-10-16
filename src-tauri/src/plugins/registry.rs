//! `plugins.sqlite`: the user's enable decisions plus a small meta table.
//! Decisions live here, never in synced settings, so a plugin choice on one
//! machine never turns code on somewhere else.
//!
//! Safe mode: the renderer writes a "loading plugins" marker before it
//! activates anything and clears it once the app has been up for 10 s. A
//! marker left over at the next launch, or a registry that won't open, starts
//! with every non-bundled plugin paused until the user resumes them.

use rusqlite::{params, Connection};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Mutex,
};

pub const REGISTRY_FILE: &str = "plugins.sqlite";
pub const LOADING_MARKER: &str = "plugins.loading";

pub struct Registry {
    conn: Mutex<Connection>,
}

impl Registry {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS decisions(id TEXT PRIMARY KEY, enabled INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )
        .map_err(|e| e.to_string())?;
        // A quick integrity probe: a corrupt file opens fine and fails here.
        conn.query_row("SELECT count(*) FROM decisions", [], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn decisions(&self) -> Result<BTreeMap<String, bool>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id, enabled FROM decisions")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0)))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), String> {
        if !super::valid_id(id) {
            return Err(format!("\"{id}\" is not a valid plugin id"));
        }
        self.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO decisions(id, enabled) VALUES (?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET enabled = ?2",
                params![id, enabled as i64],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub fn forget(&self, id: &str) -> Result<(), String> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM decisions WHERE id = ?1", params![id])
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SafeModeStatus {
    /// Non-bundled plugins start paused for this launch.
    pub paused: bool,
    /// Why, in plain words; `None` when not paused.
    pub reason: Option<String>,
    pub disabled_by_env: bool,
}

/// Launch-time state, computed once and then held for the process lifetime.
pub struct SafeMode {
    marker: PathBuf,
    status: Mutex<SafeModeStatus>,
}

impl SafeMode {
    /// Inspect the leftover marker (from the previous launch) and the registry.
    pub fn detect(data_dir: &Path, registry_error: Option<&str>) -> Self {
        let marker = data_dir.join(LOADING_MARKER);
        let reason = if let Some(error) = registry_error {
            Some(format!(
                "The plugin registry could not be opened ({error}), so plugins you installed are paused."
            ))
        } else if marker.exists() {
            Some(
                "Codemux didn't finish starting last time while plugins were loading, so plugins you installed are paused."
                    .to_string(),
            )
        } else {
            None
        };
        Self {
            marker,
            status: Mutex::new(SafeModeStatus {
                paused: reason.is_some(),
                reason,
                disabled_by_env: super::disabled_by_env(),
            }),
        }
    }

    pub fn status(&self) -> SafeModeStatus {
        self.status.lock().unwrap().clone()
    }

    /// The renderer is about to activate plugins.
    pub fn begin_loading(&self) -> Result<(), String> {
        std::fs::write(&self.marker, b"loading").map_err(|e| e.to_string())
    }

    /// The app stayed up: this launch was clean.
    pub fn startup_ok(&self) -> Result<(), String> {
        match std::fs::remove_file(&self.marker) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// "Resume plugins" in the banner.
    pub fn resume(&self) {
        let mut status = self.status.lock().unwrap();
        status.paused = false;
        status.reason = None;
    }
}
