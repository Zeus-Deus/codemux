use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

const SCHEMA_VERSION: u32 = 4;

pub struct DatabaseStore {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentProject {
    pub path: String,
    pub name: String,
    pub last_opened_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenFlowHistoryEntry {
    pub run_id: String,
    pub title: Option<String>,
    pub goal: Option<String>,
    pub status: Option<String>,
    pub agent_count: Option<i32>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

/// Persisted record for an agent-chat session so the history dropdown
/// can reopen prior chats after the app restarts. One row per pane's
/// lifetime thread_id; the `sdk_session_id` column carries the Claude
/// Agent SDK session UUID (populated once the SDK's first message
/// lands) and is what the "resume" path feeds back into
/// `StartSessionInput::resume_cursor`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChatSessionRecord {
    pub thread_id: String,
    pub sdk_session_id: Option<String>,
    pub workspace_id: String,
    pub cwd: Option<String>,
    pub provider: String,
    pub title: Option<String>,
    pub created_at: String,
    pub last_active_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectScripts {
    #[serde(default)]
    pub setup: Vec<String>,
    #[serde(default)]
    pub teardown: Vec<String>,
    #[serde(default)]
    pub run: Option<String>,
    #[serde(default)]
    pub worktree_includes: Vec<String>,
}

fn database_path() -> Option<PathBuf> {
    let config = dirs::config_dir()?;
    Some(config.join(crate::APP_DIR_NAME).join("codemux.db"))
}

fn create_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS settings (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id TEXT NOT NULL DEFAULT 'local',
            key TEXT NOT NULL,
            value TEXT NOT NULL,
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(user_id, key)
        );

        CREATE TABLE IF NOT EXISTS projects (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id TEXT NOT NULL DEFAULT 'local',
            name TEXT NOT NULL,
            path TEXT NOT NULL,
            color TEXT,
            tab_order INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(user_id, path)
        );

        CREATE TABLE IF NOT EXISTS workspace_state (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id TEXT NOT NULL DEFAULT 'local',
            workspace_id TEXT NOT NULL,
            project_id INTEGER REFERENCES projects(id),
            tab_order INTEGER NOT NULL DEFAULT 0,
            is_collapsed INTEGER NOT NULL DEFAULT 0,
            last_active_at TEXT,
            UNIQUE(user_id, workspace_id)
        );

        CREATE TABLE IF NOT EXISTS ui_state (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id TEXT NOT NULL DEFAULT 'local',
            key TEXT NOT NULL,
            value TEXT NOT NULL,
            UNIQUE(user_id, key)
        );

        CREATE TABLE IF NOT EXISTS recent_projects (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id TEXT NOT NULL DEFAULT 'local',
            path TEXT NOT NULL,
            name TEXT NOT NULL,
            last_opened_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(user_id, path)
        );

        CREATE TABLE IF NOT EXISTS openflow_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id TEXT NOT NULL DEFAULT 'local',
            run_id TEXT NOT NULL,
            title TEXT,
            goal TEXT,
            status TEXT,
            agent_count INTEGER,
            started_at TEXT,
            completed_at TEXT,
            UNIQUE(run_id)
        );

        CREATE TABLE IF NOT EXISTS auth_tokens (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            encrypted_data BLOB NOT NULL,
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS agent_chat_sessions (
            thread_id TEXT PRIMARY KEY,
            sdk_session_id TEXT,
            workspace_id TEXT NOT NULL,
            cwd TEXT,
            provider TEXT NOT NULL,
            title TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_active_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_agent_chat_sessions_workspace
            ON agent_chat_sessions(workspace_id, last_active_at DESC);
        CREATE INDEX IF NOT EXISTS idx_agent_chat_sessions_cwd
            ON agent_chat_sessions(cwd, last_active_at DESC);

        CREATE TABLE IF NOT EXISTS agent_chat_messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            thread_id TEXT NOT NULL,
            payload TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (thread_id)
                REFERENCES agent_chat_sessions(thread_id)
                ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_agent_chat_messages_thread
            ON agent_chat_messages(thread_id, id ASC);

        -- Hosts (Step 2 of cloud push — Settings → Hosts pane data model).
        --
        -- Each row is a user-defined SSH target plus a friendly name. The
        -- workspace will eventually carry a `host_id` pointing at one of
        -- these (or NULL meaning local). SSH credentials are NOT stored
        -- here and never leave the device — they live in ~/.ssh/. This
        -- table holds only the *identity* of the remote box.
        --
        -- `server_id` is the row id assigned by the API when this host
        -- syncs to the cloud, used to correlate local <-> server rows on
        -- merge. NULL until the first successful push.
        --
        -- `deleted_at` is a soft-delete tombstone so deletions sync
        -- cleanly: we keep the row locally with a deletion timestamp,
        -- push the delete, then the next pull will see it gone from
        -- the server and we can hard-delete locally. Matches the
        -- pattern Vexis uses for voice data lifecycle.
        --
        -- `dirty` flag mirrors the settings-sync model: 1 means the
        -- local row has unpushed changes, 0 means it matches the
        -- last-known server state. Lets `hosts_sync` push only what
        -- changed.
        CREATE TABLE IF NOT EXISTS hosts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id TEXT NOT NULL DEFAULT 'local',
            server_id TEXT,
            name TEXT NOT NULL,
            ssh_target TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            deleted_at TEXT,
            dirty INTEGER NOT NULL DEFAULT 1,
            UNIQUE(user_id, server_id)
        );

        CREATE INDEX IF NOT EXISTS idx_hosts_user
            ON hosts(user_id, deleted_at);
        CREATE INDEX IF NOT EXISTS idx_hosts_dirty
            ON hosts(user_id, dirty)
            WHERE dirty = 1;
        ",
    )
    .map_err(|e| format!("Failed to create database schema: {e}"))?;

    // Set (or advance) schema version. `IF NOT EXISTS` on every CREATE
    // above means re-running this on a v1 DB silently adds the new
    // table/indexes, so we just need to bump the stored version.
    let count: i32 = conn
        .query_row("SELECT COUNT(*) FROM schema_version", [], |row| row.get(0))
        .map_err(|e| format!("Failed to check schema version: {e}"))?;

    if count == 0 {
        conn.execute(
            "INSERT INTO schema_version (version) VALUES (?1)",
            params![SCHEMA_VERSION],
        )
        .map_err(|e| format!("Failed to set schema version: {e}"))?;
    } else {
        conn.execute(
            "UPDATE schema_version SET version = ?1",
            params![SCHEMA_VERSION],
        )
        .map_err(|e| format!("Failed to bump schema version: {e}"))?;
    }

    Ok(())
}

fn open_connection(path: &std::path::Path) -> Result<Connection, String> {
    let conn = Connection::open(path)
        .map_err(|e| format!("Failed to open database at {}: {e}", path.display()))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
        .map_err(|e| format!("Failed to set PRAGMA: {e}"))?;
    Ok(conn)
}

pub fn init_database() -> Result<DatabaseStore, String> {
    let db_path = database_path().ok_or("Could not determine config directory")?;

    // Ensure parent directory exists
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create config directory: {e}"))?;
    }

    let conn = open_connection(&db_path)?;
    create_schema(&conn)?;

    eprintln!(
        "[codemux::database] SQLite initialized at {}",
        db_path.display()
    );

    Ok(DatabaseStore {
        conn: Mutex::new(conn),
    })
}

impl DatabaseStore {
    /// Create an in-memory database (fallback if file DB fails, and for tests).
    pub fn new_in_memory() -> Self {
        let conn = Connection::open_in_memory().expect("Failed to open in-memory database");
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        create_schema(&conn).unwrap();
        Self {
            conn: Mutex::new(conn),
        }
    }
}

#[cfg(test)]
pub fn init_test_database() -> DatabaseStore {
    let conn = Connection::open_in_memory().expect("Failed to open in-memory database");
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    create_schema(&conn).unwrap();
    DatabaseStore {
        conn: Mutex::new(conn),
    }
}

// ── Settings ──

impl DatabaseStore {
    pub fn get_setting(&self, key: &str) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT value FROM settings WHERE user_id = 'local' AND key = ?1",
            params![key],
            |row| row.get(0),
        )
        .ok()
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO settings (user_id, key, value, updated_at) VALUES ('local', ?1, ?2, datetime('now'))
             ON CONFLICT(user_id, key) DO UPDATE SET value = ?2, updated_at = datetime('now')",
            params![key, value],
        )
        .map_err(|e| format!("Failed to set setting: {e}"))?;
        Ok(())
    }

    pub fn delete_setting(&self, key: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM settings WHERE user_id = 'local' AND key = ?1",
            params![key],
        )
        .map_err(|e| format!("Failed to delete setting: {e}"))?;
        Ok(())
    }

    pub fn get_all_settings(&self) -> HashMap<String, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT key, value FROM settings WHERE user_id = 'local'")
            .unwrap();
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .unwrap();
        rows.filter_map(|r| r.ok()).collect()
    }

    // ── UI State ──

    pub fn get_ui_state(&self, key: &str) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT value FROM ui_state WHERE user_id = 'local' AND key = ?1",
            params![key],
            |row| row.get(0),
        )
        .ok()
    }

    pub fn set_ui_state(&self, key: &str, value: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO ui_state (user_id, key, value) VALUES ('local', ?1, ?2)
             ON CONFLICT(user_id, key) DO UPDATE SET value = ?2",
            params![key, value],
        )
        .map_err(|e| format!("Failed to set ui_state: {e}"))?;
        Ok(())
    }

    pub fn delete_ui_state(&self, key: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM ui_state WHERE user_id = 'local' AND key = ?1",
            params![key],
        )
        .map_err(|e| format!("Failed to delete ui_state: {e}"))?;
        Ok(())
    }

    // ── Project Scripts ──

    pub fn get_project_scripts(&self, project_root: &str) -> Option<ProjectScripts> {
        let key = format!("project.scripts:{project_root}");
        self.get_setting(&key)
            .and_then(|v| serde_json::from_str(&v).ok())
    }

    pub fn set_project_scripts(
        &self,
        project_root: &str,
        scripts: &ProjectScripts,
    ) -> Result<(), String> {
        let key = format!("project.scripts:{project_root}");
        let value = serde_json::to_string(scripts).map_err(|e| e.to_string())?;
        self.set_setting(&key, &value)
    }

    // ── Recent Projects ──

    pub fn add_recent_project(&self, path: &str, name: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO recent_projects (user_id, path, name, last_opened_at) VALUES ('local', ?1, ?2, datetime('now'))
             ON CONFLICT(user_id, path) DO UPDATE SET name = ?2, last_opened_at = datetime('now')",
            params![path, name],
        )
        .map_err(|e| format!("Failed to add recent project: {e}"))?;
        Ok(())
    }

    pub fn get_recent_projects(&self, limit: u32) -> Vec<RecentProject> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT path, name, last_opened_at FROM recent_projects
                 WHERE user_id = 'local' ORDER BY last_opened_at DESC, id DESC LIMIT ?1",
            )
            .unwrap();
        stmt.query_map(params![limit], |row| {
            Ok(RecentProject {
                path: row.get(0)?,
                name: row.get(1)?,
                last_opened_at: row.get(2)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    // ── OpenFlow History ──

    pub fn save_openflow_run(
        &self,
        run_id: &str,
        title: Option<&str>,
        goal: Option<&str>,
        status: Option<&str>,
        agent_count: Option<i32>,
        started_at: Option<&str>,
        completed_at: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO openflow_history (user_id, run_id, title, goal, status, agent_count, started_at, completed_at)
             VALUES ('local', ?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(run_id) DO UPDATE SET title = ?2, goal = ?3, status = ?4, agent_count = ?5, started_at = ?6, completed_at = ?7",
            params![run_id, title, goal, status, agent_count, started_at, completed_at],
        )
        .map_err(|e| format!("Failed to save openflow run: {e}"))?;
        Ok(())
    }

    pub fn get_openflow_history(&self, limit: u32) -> Vec<OpenFlowHistoryEntry> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT run_id, title, goal, status, agent_count, started_at, completed_at
                 FROM openflow_history WHERE user_id = 'local'
                 ORDER BY started_at DESC LIMIT ?1",
            )
            .unwrap();
        stmt.query_map(params![limit], |row| {
            Ok(OpenFlowHistoryEntry {
                run_id: row.get(0)?,
                title: row.get(1)?,
                goal: row.get(2)?,
                status: row.get(3)?,
                agent_count: row.get(4)?,
                started_at: row.get(5)?,
                completed_at: row.get(6)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }
}

// ── Hosts (Step 2 of cloud push) ──
//
// CRUD over the `hosts` table. Soft-delete semantics: `delete_host`
// stamps `deleted_at` rather than removing the row, so the sync layer
// has a tombstone to push. `purge_synced_deletes` is called by the
// sync layer after a successful round-trip to physically remove
// already-acknowledged tombstones.
//
// SSH credentials live in `~/.ssh/`, never here. This table holds
// identity only.

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HostRecord {
    pub id: i64,
    pub server_id: Option<String>,
    pub name: String,
    pub ssh_target: String,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
    pub dirty: bool,
}

impl DatabaseStore {
    /// Insert a new host. Marked dirty so the next sync round-trip pushes it.
    pub fn insert_host(&self, name: &str, ssh_target: &str) -> Result<HostRecord, String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO hosts (user_id, name, ssh_target, dirty)
             VALUES ('local', ?1, ?2, 1)",
            params![name, ssh_target],
        )
        .map_err(|e| format!("Failed to insert host: {e}"))?;
        let id = conn.last_insert_rowid();
        conn.query_row(
            "SELECT id, server_id, name, ssh_target, created_at, updated_at, deleted_at, dirty
             FROM hosts WHERE id = ?1",
            params![id],
            row_to_host,
        )
        .map_err(|e| format!("Failed to re-read inserted host: {e}"))
    }

    /// Return all non-deleted hosts for the local user.
    pub fn list_hosts(&self) -> Vec<HostRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT id, server_id, name, ssh_target, created_at, updated_at, deleted_at, dirty
             FROM hosts
             WHERE user_id = 'local' AND deleted_at IS NULL
             ORDER BY name COLLATE NOCASE ASC",
        ) {
            Ok(s) => s,
            Err(error) => {
                eprintln!("[codemux::database] list_hosts prepare failed: {error}");
                return Vec::new();
            }
        };
        let rows = match stmt.query_map([], row_to_host) {
            Ok(r) => r,
            Err(error) => {
                eprintln!("[codemux::database] list_hosts query_map failed: {error}");
                return Vec::new();
            }
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    /// Return every host row including soft-deleted tombstones — used
    /// by the sync layer to push pending deletions to the server.
    pub fn list_hosts_for_sync(&self) -> Vec<HostRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT id, server_id, name, ssh_target, created_at, updated_at, deleted_at, dirty
             FROM hosts WHERE user_id = 'local'",
        ) {
            Ok(s) => s,
            Err(error) => {
                eprintln!("[codemux::database] list_hosts_for_sync prepare failed: {error}");
                return Vec::new();
            }
        };
        let rows = match stmt.query_map([], row_to_host) {
            Ok(r) => r,
            Err(error) => {
                eprintln!("[codemux::database] list_hosts_for_sync query_map failed: {error}");
                return Vec::new();
            }
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    /// Return only rows with unpushed changes (dirty=1). Used by the
    /// sync layer's "push my deltas" step so we don't re-upload rows
    /// that already match the server.
    pub fn list_dirty_hosts(&self) -> Vec<HostRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT id, server_id, name, ssh_target, created_at, updated_at, deleted_at, dirty
             FROM hosts WHERE user_id = 'local' AND dirty = 1",
        ) {
            Ok(s) => s,
            Err(error) => {
                eprintln!("[codemux::database] list_dirty_hosts prepare failed: {error}");
                return Vec::new();
            }
        };
        let rows = match stmt.query_map([], row_to_host) {
            Ok(r) => r,
            Err(error) => {
                eprintln!("[codemux::database] list_dirty_hosts query_map failed: {error}");
                return Vec::new();
            }
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    pub fn update_host(
        &self,
        id: i64,
        name: &str,
        ssh_target: &str,
    ) -> Result<HostRecord, String> {
        let conn = self.conn.lock().unwrap();
        let affected = conn
            .execute(
                "UPDATE hosts
                 SET name = ?1, ssh_target = ?2, updated_at = datetime('now'), dirty = 1
                 WHERE id = ?3 AND user_id = 'local' AND deleted_at IS NULL",
                params![name, ssh_target, id],
            )
            .map_err(|e| format!("Failed to update host: {e}"))?;
        if affected == 0 {
            return Err(format!("No host with id {id}"));
        }
        conn.query_row(
            "SELECT id, server_id, name, ssh_target, created_at, updated_at, deleted_at, dirty
             FROM hosts WHERE id = ?1",
            params![id],
            row_to_host,
        )
        .map_err(|e| format!("Failed to re-read updated host: {e}"))
    }

    /// Soft-delete: stamp `deleted_at` and mark dirty so the next sync
    /// pushes the tombstone. The row stays in the DB until
    /// `purge_synced_deletes` runs.
    pub fn delete_host(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let affected = conn
            .execute(
                "UPDATE hosts
                 SET deleted_at = datetime('now'), updated_at = datetime('now'), dirty = 1
                 WHERE id = ?1 AND user_id = 'local' AND deleted_at IS NULL",
                params![id],
            )
            .map_err(|e| format!("Failed to soft-delete host: {e}"))?;
        if affected == 0 {
            return Err(format!("No host with id {id}"));
        }
        Ok(())
    }

    /// Clear the dirty flag on a host after a successful push. Optionally
    /// stamp `server_id` if this was the first upload.
    pub fn mark_host_synced(
        &self,
        id: i64,
        server_id: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        if let Some(sid) = server_id {
            conn.execute(
                "UPDATE hosts SET dirty = 0, server_id = ?1 WHERE id = ?2",
                params![sid, id],
            )
            .map_err(|e| format!("Failed to mark host synced: {e}"))?;
        } else {
            conn.execute("UPDATE hosts SET dirty = 0 WHERE id = ?1", params![id])
                .map_err(|e| format!("Failed to mark host synced: {e}"))?;
        }
        Ok(())
    }

    /// Hard-delete tombstones the server has confirmed it removed. Safe
    /// to call after a successful sync round-trip; no-op when nothing
    /// matches.
    pub fn purge_acknowledged_deletes(&self) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM hosts WHERE deleted_at IS NOT NULL AND dirty = 0",
            [],
        )
        .map_err(|e| format!("Failed to purge tombstones: {e}"))?;
        Ok(())
    }

    /// Upsert a row received from the server. If a local row already
    /// exists with the same `server_id`, update in place; otherwise
    /// insert. Always marked `dirty = 0` because this row came from the
    /// server.
    pub fn upsert_host_from_server(
        &self,
        server_id: &str,
        name: &str,
        ssh_target: &str,
        created_at: &str,
        updated_at: &str,
        deleted_at: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        // Try to update an existing row first.
        let updated = conn
            .execute(
                "UPDATE hosts
                 SET name = ?1, ssh_target = ?2, created_at = ?3, updated_at = ?4,
                     deleted_at = ?5, dirty = 0
                 WHERE user_id = 'local' AND server_id = ?6",
                params![name, ssh_target, created_at, updated_at, deleted_at, server_id],
            )
            .map_err(|e| format!("Failed to update host from server: {e}"))?;
        if updated == 0 {
            conn.execute(
                "INSERT INTO hosts (user_id, server_id, name, ssh_target, created_at, updated_at, deleted_at, dirty)
                 VALUES ('local', ?1, ?2, ?3, ?4, ?5, ?6, 0)",
                params![server_id, name, ssh_target, created_at, updated_at, deleted_at],
            )
            .map_err(|e| format!("Failed to insert host from server: {e}"))?;
        }
        Ok(())
    }
}

fn row_to_host(row: &rusqlite::Row<'_>) -> rusqlite::Result<HostRecord> {
    let dirty_int: i64 = row.get(7)?;
    Ok(HostRecord {
        id: row.get(0)?,
        server_id: row.get(1)?,
        name: row.get(2)?,
        ssh_target: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        deleted_at: row.get(6)?,
        dirty: dirty_int != 0,
    })
}

// ── Agent Chat Sessions ──
//
// Persistence for the chat-history dropdown. One row per thread the
// user has ever opened in a given workspace/cwd. Rows are cheap
// (text-only, ~200 bytes each) so we never prune automatically — the
// user deletes explicitly from the dropdown.

impl DatabaseStore {
    /// Insert or refresh a session row. Called from
    /// `agent_chat_start_session`: on a brand-new chat we INSERT, on a
    /// silent restart (same thread_id after migrate) the ON CONFLICT
    /// bumps `last_active_at` and preserves `sdk_session_id`/`title`.
    pub fn upsert_agent_chat_session(
        &self,
        thread_id: &str,
        workspace_id: &str,
        cwd: Option<&str>,
        provider: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO agent_chat_sessions
                 (thread_id, workspace_id, cwd, provider, created_at, last_active_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'), datetime('now'))
             ON CONFLICT(thread_id) DO UPDATE SET
                 workspace_id = ?2,
                 cwd = ?3,
                 provider = ?4,
                 last_active_at = datetime('now')",
            params![thread_id, workspace_id, cwd, provider],
        )
        .map_err(|e| format!("Failed to upsert agent_chat_session: {e}"))?;
        Ok(())
    }

    /// Copy metadata (title, sdk_session_id, created_at) from an old
    /// thread id to a new one. Used on silent restart so the session
    /// keeps its history-dropdown identity after
    /// `migrateThreadId`. The caller is expected to `upsert` the new
    /// row first; this just carries forward the human-facing fields.
    pub fn migrate_agent_chat_session(
        &self,
        old_thread_id: &str,
        new_thread_id: &str,
    ) -> Result<(), String> {
        if old_thread_id == new_thread_id {
            return Ok(());
        }
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agent_chat_sessions SET
                 title = COALESCE(
                     (SELECT title FROM agent_chat_sessions WHERE thread_id = ?1),
                     title
                 ),
                 sdk_session_id = COALESCE(
                     (SELECT sdk_session_id FROM agent_chat_sessions WHERE thread_id = ?1),
                     sdk_session_id
                 ),
                 created_at = COALESCE(
                     (SELECT created_at FROM agent_chat_sessions WHERE thread_id = ?1),
                     created_at
                 )
             WHERE thread_id = ?2",
            params![old_thread_id, new_thread_id],
        )
        .map_err(|e| format!("Failed to migrate agent_chat_session: {e}"))?;
        // Move persisted messages over BEFORE dropping the source row,
        // otherwise ON DELETE CASCADE would wipe the transcript history
        // we just promised to carry forward.
        conn.execute(
            "UPDATE agent_chat_messages SET thread_id = ?2 WHERE thread_id = ?1",
            params![old_thread_id, new_thread_id],
        )
        .map_err(|e| format!("Failed to migrate agent_chat_messages: {e}"))?;
        conn.execute(
            "DELETE FROM agent_chat_sessions WHERE thread_id = ?1",
            params![old_thread_id],
        )
        .map_err(|e| format!("Failed to drop migrated source row: {e}"))?;
        Ok(())
    }

    /// Record the Claude Agent SDK session UUID once the sidecar's
    /// `sdk-session-id` notification has resolved. This is the value
    /// that later feeds `StartSessionInput::resume_cursor` when the
    /// user reopens the chat from the history dropdown.
    pub fn set_agent_chat_sdk_session_id(
        &self,
        thread_id: &str,
        sdk_session_id: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agent_chat_sessions
                 SET sdk_session_id = ?2, last_active_at = datetime('now')
                 WHERE thread_id = ?1",
            params![thread_id, sdk_session_id],
        )
        .map_err(|e| format!("Failed to set sdk_session_id: {e}"))?;
        Ok(())
    }

    /// Set (or replace) the dropdown title for a session. Called
    /// once from the first-turn auto-title path and again any time
    /// the user renames the session from the dropdown.
    pub fn set_agent_chat_title(
        &self,
        thread_id: &str,
        title: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agent_chat_sessions
                 SET title = ?2, last_active_at = datetime('now')
                 WHERE thread_id = ?1",
            params![thread_id, title],
        )
        .map_err(|e| format!("Failed to set title: {e}"))?;
        Ok(())
    }

    /// Returns the current title for a session, or None if the
    /// session is absent / has no title yet. Used by the auto-title
    /// path to check "should I generate a title from this turn".
    pub fn get_agent_chat_title(&self, thread_id: &str) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT title FROM agent_chat_sessions WHERE thread_id = ?1",
            params![thread_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
    }

    /// Bump `last_active_at` so an active session floats to the top
    /// of the dropdown. Called on every user turn.
    pub fn touch_agent_chat_session(&self, thread_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agent_chat_sessions
                 SET last_active_at = datetime('now')
                 WHERE thread_id = ?1",
            params![thread_id],
        )
        .map_err(|e| format!("Failed to touch session: {e}"))?;
        Ok(())
    }

    /// List sessions for the dropdown, ordered by recency. Scoped to
    /// a single workspace. When `cwd` is Some the list is further
    /// filtered to sessions opened from that exact directory so the
    /// dropdown matches the pane the user is looking at.
    pub fn list_agent_chat_sessions(
        &self,
        workspace_id: &str,
        cwd: Option<&str>,
        limit: u32,
    ) -> Vec<AgentChatSessionRecord> {
        let conn = self.conn.lock().unwrap();
        let map_row = |row: &rusqlite::Row<'_>| {
            Ok(AgentChatSessionRecord {
                thread_id: row.get(0)?,
                sdk_session_id: row.get(1)?,
                workspace_id: row.get(2)?,
                cwd: row.get(3)?,
                provider: row.get(4)?,
                title: row.get(5)?,
                created_at: row.get(6)?,
                last_active_at: row.get(7)?,
            })
        };
        // Only surface rows that actually have an sdk_session_id —
        // without one the Claude SDK cannot resume the conversation
        // so offering them in the dropdown leads to a dead-end toast.
        // Rows are created with null sdk_session_id and get the UUID
        // once `ResumeCursorUpdated` fires (after the first SDK
        // message), so this filter cleanly hides transient rows from
        // silent restarts that never got interacted with.
        if let Some(cwd) = cwd {
            let mut stmt = match conn.prepare(
                "SELECT thread_id, sdk_session_id, workspace_id, cwd, provider, title, created_at, last_active_at
                 FROM agent_chat_sessions
                 WHERE workspace_id = ?1 AND cwd = ?2 AND sdk_session_id IS NOT NULL
                 ORDER BY last_active_at DESC LIMIT ?3",
            ) {
                Ok(s) => s,
                Err(_) => return Vec::new(),
            };
            stmt.query_map(params![workspace_id, cwd, limit], map_row)
                .map(|iter| iter.filter_map(|r| r.ok()).collect())
                .unwrap_or_default()
        } else {
            let mut stmt = match conn.prepare(
                "SELECT thread_id, sdk_session_id, workspace_id, cwd, provider, title, created_at, last_active_at
                 FROM agent_chat_sessions
                 WHERE workspace_id = ?1 AND sdk_session_id IS NOT NULL
                 ORDER BY last_active_at DESC LIMIT ?2",
            ) {
                Ok(s) => s,
                Err(_) => return Vec::new(),
            };
            stmt.query_map(params![workspace_id, limit], map_row)
                .map(|iter| iter.filter_map(|r| r.ok()).collect())
                .unwrap_or_default()
        }
    }

    /// De-duplicate sessions that share an `sdk_session_id`. When the
    /// user resumes a past chat we start a NEW provider session with
    /// a new `thread_id`, but its SDK session id matches an existing
    /// row's. Without cleanup the dropdown would eventually show
    /// duplicate entries for the same logical conversation. This
    /// collapses all rows sharing an `sdk_session_id` into the most
    /// recent one, carrying forward the best title / earliest
    /// created_at.
    pub fn collapse_duplicate_agent_chat_sessions(
        &self,
        sdk_session_id: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        // Pick the most-recently-active row as the survivor.
        let survivor: Option<String> = conn
            .query_row(
                "SELECT thread_id FROM agent_chat_sessions
                 WHERE sdk_session_id = ?1
                 ORDER BY last_active_at DESC LIMIT 1",
                params![sdk_session_id],
                |row| row.get(0),
            )
            .ok();
        let Some(survivor) = survivor else {
            return Ok(());
        };
        // Merge identity fields: keep the earliest created_at and
        // any non-null title across the whole set onto the survivor.
        conn.execute(
            "UPDATE agent_chat_sessions
                 SET created_at = COALESCE(
                     (SELECT MIN(created_at) FROM agent_chat_sessions
                      WHERE sdk_session_id = ?1),
                     created_at
                 ),
                 title = COALESCE(
                     title,
                     (SELECT title FROM agent_chat_sessions
                      WHERE sdk_session_id = ?1 AND title IS NOT NULL
                      ORDER BY last_active_at DESC LIMIT 1)
                 )
             WHERE thread_id = ?2",
            params![sdk_session_id, survivor],
        )
        .map_err(|e| format!("Failed to merge duplicate sessions: {e}"))?;
        // Move persisted messages from the non-survivors onto the
        // survivor BEFORE the DELETE — otherwise ON DELETE CASCADE
        // wipes the transcript history of the prior thread_ids whose
        // messages we want to keep visible after resume.
        conn.execute(
            "UPDATE agent_chat_messages
             SET thread_id = ?2
             WHERE thread_id IN (
                 SELECT thread_id FROM agent_chat_sessions
                 WHERE sdk_session_id = ?1 AND thread_id != ?2
             )",
            params![sdk_session_id, survivor],
        )
        .map_err(|e| format!("Failed to migrate duplicate session messages: {e}"))?;
        // Delete the non-survivors.
        conn.execute(
            "DELETE FROM agent_chat_sessions
             WHERE sdk_session_id = ?1 AND thread_id != ?2",
            params![sdk_session_id, survivor],
        )
        .map_err(|e| format!("Failed to drop duplicate sessions: {e}"))?;
        Ok(())
    }

    /// Delete a session row. Idempotent.
    pub fn delete_agent_chat_session(&self, thread_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM agent_chat_sessions WHERE thread_id = ?1",
            params![thread_id],
        )
        .map_err(|e| format!("Failed to delete session: {e}"))?;
        Ok(())
    }

    /// Fetch a single session record by thread_id.
    pub fn get_agent_chat_session(
        &self,
        thread_id: &str,
    ) -> Option<AgentChatSessionRecord> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT thread_id, sdk_session_id, workspace_id, cwd, provider, title, created_at, last_active_at
             FROM agent_chat_sessions WHERE thread_id = ?1",
            params![thread_id],
            |row| {
                Ok(AgentChatSessionRecord {
                    thread_id: row.get(0)?,
                    sdk_session_id: row.get(1)?,
                    workspace_id: row.get(2)?,
                    cwd: row.get(3)?,
                    provider: row.get(4)?,
                    title: row.get(5)?,
                    created_at: row.get(6)?,
                    last_active_at: row.get(7)?,
                })
            },
        )
        .ok()
    }
}

// ── Agent Chat Messages ──
//
// Per-message persistence so the SessionSelector "Resume" path can
// rehydrate the visible transcript, not just the SDK's server-side
// context. Rows are JSON-serialized envelopes — either a canonical
// `ProviderRuntimeEvent` or a synthetic `{type: "user_message", ...}`
// record (since user messages live only on the client side and never
// pass through the provider stream). Frontend hydration replays each
// payload through the same reducer that handles live events, so the
// rebuilt transcript is byte-identical to the original render.
//
// FK to agent_chat_sessions(thread_id) ON DELETE CASCADE means an
// explicit `delete_agent_chat_session` cleanly drops history. The two
// silent-restart / collapse paths above explicitly MOVE messages onto
// the survivor row before the delete, so logical-conversation merges
// never lose history.

impl DatabaseStore {
    /// Append a single message envelope. `payload_json` must be a
    /// fully-serialized JSON string — typically the result of
    /// `serde_json::to_string(&ProviderRuntimeEvent::…)` or a synthetic
    /// `{"type":"user_message","text":"…"}` record. Best-effort: rows
    /// for an unknown thread_id (FK violation) are silently dropped
    /// rather than crashing the event-bridge loop.
    pub fn append_agent_chat_message(
        &self,
        thread_id: &str,
        payload_json: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        match conn.execute(
            "INSERT INTO agent_chat_messages (thread_id, payload)
             VALUES (?1, ?2)",
            params![thread_id, payload_json],
        ) {
            Ok(_) => Ok(()),
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY =>
            {
                // Parent session row was already deleted (rare race
                // between forward_event and delete_agent_chat_session).
                // Drop the message silently — it is exactly the
                // history the user just asked us to forget.
                Ok(())
            }
            Err(e) => Err(format!("Failed to append agent_chat_message: {e}")),
        }
    }

    /// Return every persisted message for a thread, ordered by
    /// insertion time (the autoincrement `id`). Each element is the
    /// raw JSON payload — the frontend parses and dispatches it.
    pub fn list_agent_chat_messages(&self, thread_id: &str) -> Vec<String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT payload FROM agent_chat_messages
             WHERE thread_id = ?1
             ORDER BY id ASC",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map(params![thread_id], |row| row.get::<_, String>(0))
            .map(|iter| iter.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// Drop every persisted message for a thread without touching the
    /// session row. Used when the user picks "New Chat" against an
    /// existing thread (we'd rather not pollute history with the
    /// prior conversation's messages once they've been explicitly
    /// abandoned). Idempotent.
    #[cfg(test)]
    pub fn delete_agent_chat_messages_for_thread(
        &self,
        thread_id: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM agent_chat_messages WHERE thread_id = ?1",
            params![thread_id],
        )
        .map_err(|e| format!("Failed to delete messages: {e}"))?;
        Ok(())
    }
}

// ── Auth Tokens ──

impl DatabaseStore {
    pub fn save_auth_token(&self, encrypted_data: &[u8]) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO auth_tokens (id, encrypted_data, updated_at) VALUES (1, ?1, datetime('now'))",
            params![encrypted_data],
        )
        .map_err(|e| format!("Failed to save auth token: {e}"))?;
        Ok(())
    }

    pub fn load_auth_token(&self) -> Option<Vec<u8>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT encrypted_data FROM auth_tokens WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .ok()
    }

    pub fn clear_auth_token(&self) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute("DELETE FROM auth_tokens WHERE id = 1", []);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_crud() {
        let db = init_test_database();

        // Get non-existent setting
        assert_eq!(db.get_setting("theme"), None);

        // Set and get
        db.set_setting("theme", "dark").unwrap();
        assert_eq!(db.get_setting("theme"), Some("dark".into()));

        // Update
        db.set_setting("theme", "light").unwrap();
        assert_eq!(db.get_setting("theme"), Some("light".into()));

        // Delete
        db.delete_setting("theme").unwrap();
        assert_eq!(db.get_setting("theme"), None);
    }

    #[test]
    fn get_all_settings() {
        let db = init_test_database();

        db.set_setting("a", "1").unwrap();
        db.set_setting("b", "2").unwrap();
        db.set_setting("c", "3").unwrap();

        let all = db.get_all_settings();
        assert_eq!(all.len(), 3);
        assert_eq!(all.get("a"), Some(&"1".into()));
        assert_eq!(all.get("b"), Some(&"2".into()));
        assert_eq!(all.get("c"), Some(&"3".into()));
    }

    #[test]
    fn ui_state_crud() {
        let db = init_test_database();

        assert_eq!(db.get_ui_state("sidebar_width"), None);

        db.set_ui_state("sidebar_width", "300").unwrap();
        assert_eq!(db.get_ui_state("sidebar_width"), Some("300".into()));

        // Upsert
        db.set_ui_state("sidebar_width", "250").unwrap();
        assert_eq!(db.get_ui_state("sidebar_width"), Some("250".into()));
    }

    #[test]
    fn recent_projects_crud() {
        let db = init_test_database();

        // Empty list
        assert_eq!(db.get_recent_projects(10).len(), 0);

        // Add projects
        db.add_recent_project("/home/user/project-a", "project-a")
            .unwrap();
        db.add_recent_project("/home/user/project-b", "project-b")
            .unwrap();

        let projects = db.get_recent_projects(10);
        assert_eq!(projects.len(), 2);

        // Both projects present
        let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"project-a"));
        assert!(names.contains(&"project-b"));

        // Upsert doesn't duplicate
        db.add_recent_project("/home/user/project-a", "project-a-renamed")
            .unwrap();
        let projects = db.get_recent_projects(10);
        assert_eq!(projects.len(), 2);
        // Name was updated
        assert!(projects.iter().any(|p| p.name == "project-a-renamed"));

        // Limit
        let projects = db.get_recent_projects(1);
        assert_eq!(projects.len(), 1);
    }

    #[test]
    fn openflow_history_crud() {
        let db = init_test_database();

        assert_eq!(db.get_openflow_history(10).len(), 0);

        db.save_openflow_run(
            "run-1",
            Some("Test Run"),
            Some("Build a feature"),
            Some("completed"),
            Some(3),
            Some("2025-01-01T00:00:00"),
            Some("2025-01-01T01:00:00"),
        )
        .unwrap();

        let history = db.get_openflow_history(10);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].run_id, "run-1");
        assert_eq!(history[0].title, Some("Test Run".into()));
        assert_eq!(history[0].agent_count, Some(3));

        // Update
        db.save_openflow_run(
            "run-1",
            Some("Test Run"),
            Some("Build a feature"),
            Some("failed"),
            Some(3),
            Some("2025-01-01T00:00:00"),
            None,
        )
        .unwrap();

        let history = db.get_openflow_history(10);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].status, Some("failed".into()));
    }

    #[test]
    fn schema_version_set() {
        let db = init_test_database();
        let conn = db.conn.lock().unwrap();
        let version: u32 = conn
            .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    // ── Settings Persistence ──

    #[test]
    fn settings_default_returns_none() {
        let db = init_test_database();
        // Non-existent settings return None (app should use defaults)
        assert_eq!(db.get_setting("nonexistent_key"), None);
        assert_eq!(db.get_setting("theme"), None);
        assert_eq!(db.get_setting("notification_sound_enabled"), None);
    }

    #[test]
    fn settings_multiple_independent() {
        let db = init_test_database();

        db.set_setting("theme", "dark").unwrap();
        db.set_setting("font_size", "14").unwrap();
        db.set_setting("ai_enabled", "true").unwrap();

        // Each setting is independent
        assert_eq!(db.get_setting("theme"), Some("dark".into()));
        assert_eq!(db.get_setting("font_size"), Some("14".into()));
        assert_eq!(db.get_setting("ai_enabled"), Some("true".into()));

        // Deleting one doesn't affect others
        db.delete_setting("font_size").unwrap();
        assert_eq!(db.get_setting("theme"), Some("dark".into()));
        assert_eq!(db.get_setting("font_size"), None);
        assert_eq!(db.get_setting("ai_enabled"), Some("true".into()));

        // get_all_settings reflects deletions
        let all = db.get_all_settings();
        assert_eq!(all.len(), 2);
        assert!(!all.contains_key("font_size"));
    }

    #[test]
    fn settings_delete_nonexistent_is_ok() {
        let db = init_test_database();
        // Deleting a non-existent key should not error
        assert!(db.delete_setting("does_not_exist").is_ok());
    }

    #[test]
    fn settings_empty_and_special_values() {
        let db = init_test_database();

        // Empty string value
        db.set_setting("empty", "").unwrap();
        assert_eq!(db.get_setting("empty"), Some("".into()));

        // Value with special characters
        db.set_setting("special", "hello \"world\" 'test' \n\t").unwrap();
        assert_eq!(db.get_setting("special"), Some("hello \"world\" 'test' \n\t".into()));

        // Long value
        let long_val = "x".repeat(10_000);
        db.set_setting("long", &long_val).unwrap();
        assert_eq!(db.get_setting("long"), Some(long_val));
    }

    // ── UI State Persistence ──

    #[test]
    fn ui_state_collapse_states() {
        let db = init_test_database();

        // Save collapse states for multiple project groups
        db.set_ui_state("collapsed:project:/home/user/project-a", "true").unwrap();
        db.set_ui_state("collapsed:project:/home/user/project-b", "false").unwrap();

        assert_eq!(db.get_ui_state("collapsed:project:/home/user/project-a"), Some("true".into()));
        assert_eq!(db.get_ui_state("collapsed:project:/home/user/project-b"), Some("false".into()));
        assert_eq!(db.get_ui_state("collapsed:project:/home/user/project-c"), None);
    }

    #[test]
    fn ui_state_right_panel_width() {
        let db = init_test_database();

        db.set_ui_state("right_panel_width", "350").unwrap();
        assert_eq!(db.get_ui_state("right_panel_width"), Some("350".into()));

        // Update width
        db.set_ui_state("right_panel_width", "280").unwrap();
        assert_eq!(db.get_ui_state("right_panel_width"), Some("280".into()));
    }

    #[test]
    fn ui_state_active_workspace() {
        let db = init_test_database();

        db.set_ui_state("active_workspace", "workspace-abc123").unwrap();
        assert_eq!(db.get_ui_state("active_workspace"), Some("workspace-abc123".into()));

        // Switch workspace
        db.set_ui_state("active_workspace", "workspace-def456").unwrap();
        assert_eq!(db.get_ui_state("active_workspace"), Some("workspace-def456".into()));
    }

    #[test]
    fn ui_state_window_dimensions() {
        let db = init_test_database();

        db.set_ui_state("window_width", "1920").unwrap();
        db.set_ui_state("window_height", "1080").unwrap();
        db.set_ui_state("window_x", "100").unwrap();
        db.set_ui_state("window_y", "50").unwrap();

        assert_eq!(db.get_ui_state("window_width"), Some("1920".into()));
        assert_eq!(db.get_ui_state("window_height"), Some("1080".into()));
        assert_eq!(db.get_ui_state("window_x"), Some("100".into()));
        assert_eq!(db.get_ui_state("window_y"), Some("50".into()));

        // Update dimensions (window resized)
        db.set_ui_state("window_width", "2560").unwrap();
        db.set_ui_state("window_height", "1440").unwrap();
        assert_eq!(db.get_ui_state("window_width"), Some("2560".into()));
        assert_eq!(db.get_ui_state("window_height"), Some("1440".into()));
    }

    #[test]
    fn ui_state_overwrite_existing() {
        let db = init_test_database();

        db.set_ui_state("key", "value1").unwrap();
        assert_eq!(db.get_ui_state("key"), Some("value1".into()));

        db.set_ui_state("key", "value2").unwrap();
        assert_eq!(db.get_ui_state("key"), Some("value2".into()));

        db.set_ui_state("key", "value3").unwrap();
        assert_eq!(db.get_ui_state("key"), Some("value3".into()));
    }

    // ── Recent Projects ──

    #[test]
    fn recent_projects_ordering() {
        let db = init_test_database();

        // Add 5 projects with explicit timestamps via raw SQL to control ordering
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO recent_projects (user_id, path, name, last_opened_at) VALUES ('local', '/p1', 'p1', '2025-01-01')",
                [],
            ).unwrap();
            conn.execute(
                "INSERT INTO recent_projects (user_id, path, name, last_opened_at) VALUES ('local', '/p2', 'p2', '2025-01-03')",
                [],
            ).unwrap();
            conn.execute(
                "INSERT INTO recent_projects (user_id, path, name, last_opened_at) VALUES ('local', '/p3', 'p3', '2025-01-02')",
                [],
            ).unwrap();
        }

        let projects = db.get_recent_projects(10);
        assert_eq!(projects.len(), 3);
        // Most recent first
        assert_eq!(projects[0].name, "p2"); // 2025-01-03
        assert_eq!(projects[1].name, "p3"); // 2025-01-02
        assert_eq!(projects[2].name, "p1"); // 2025-01-01
    }

    #[test]
    fn recent_projects_many() {
        let db = init_test_database();

        // Add 20 projects
        for i in 0..20 {
            db.add_recent_project(
                &format!("/home/user/project-{i:02}"),
                &format!("project-{i:02}"),
            )
            .unwrap();
        }

        let all = db.get_recent_projects(100);
        assert_eq!(all.len(), 20);

        // Limit works
        let limited = db.get_recent_projects(5);
        assert_eq!(limited.len(), 5);

        // All unique paths
        let paths: std::collections::HashSet<&str> = all.iter().map(|p| p.path.as_str()).collect();
        assert_eq!(paths.len(), 20);
    }

    // ── OpenFlow History ──

    #[test]
    fn openflow_multiple_runs() {
        let db = init_test_database();

        for i in 0..5 {
            db.save_openflow_run(
                &format!("run-{i}"),
                Some(&format!("Run {i}")),
                Some("Test goal"),
                Some("completed"),
                Some(3),
                Some(&format!("2025-01-0{}", i + 1)),
                None,
            )
            .unwrap();
        }

        let history = db.get_openflow_history(100);
        assert_eq!(history.len(), 5);

        // Most recent first (by started_at DESC)
        assert_eq!(history[0].run_id, "run-4");
        assert_eq!(history[4].run_id, "run-0");

        // Limit
        let limited = db.get_openflow_history(2);
        assert_eq!(limited.len(), 2);
    }

    #[test]
    fn openflow_nullable_fields() {
        let db = init_test_database();

        // Save with all nulls except run_id
        db.save_openflow_run("run-null", None, None, None, None, None, None)
            .unwrap();

        let history = db.get_openflow_history(10);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].run_id, "run-null");
        assert_eq!(history[0].title, None);
        assert_eq!(history[0].goal, None);
        assert_eq!(history[0].status, None);
        assert_eq!(history[0].agent_count, None);
    }

    // ── Integration / Lifecycle ──

    #[test]
    fn lifecycle_file_persistence() {
        // Test that data survives closing and reopening the database
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        // Phase 1: Create DB and write data
        {
            let conn = open_connection(&db_path).unwrap();
            create_schema(&conn).unwrap();
            let db = DatabaseStore { conn: Mutex::new(conn) };

            db.set_setting("theme", "dark").unwrap();
            db.set_ui_state("window_width", "1920").unwrap();
            db.add_recent_project("/home/user/myapp", "myapp").unwrap();
            db.save_openflow_run("run-x", Some("Lifecycle Test"), None, Some("running"), Some(2), None, None).unwrap();
        }
        // Connection dropped here

        // Phase 2: Reopen DB and verify data survived
        {
            let conn = open_connection(&db_path).unwrap();
            create_schema(&conn).unwrap(); // Should be idempotent
            let db = DatabaseStore { conn: Mutex::new(conn) };

            assert_eq!(db.get_setting("theme"), Some("dark".into()));
            assert_eq!(db.get_ui_state("window_width"), Some("1920".into()));

            let projects = db.get_recent_projects(10);
            assert_eq!(projects.len(), 1);
            assert_eq!(projects[0].name, "myapp");

            let history = db.get_openflow_history(10);
            assert_eq!(history.len(), 1);
            assert_eq!(history[0].run_id, "run-x");
            assert_eq!(history[0].status, Some("running".into()));
        }
    }

    #[test]
    fn schema_creation_is_idempotent() {
        // Running create_schema multiple times should not error or lose data
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();

        create_schema(&conn).unwrap();

        // Insert data
        conn.execute(
            "INSERT INTO settings (user_id, key, value) VALUES ('local', 'test', 'hello')",
            [],
        ).unwrap();

        // Run schema creation again
        create_schema(&conn).unwrap();

        // Data preserved
        let val: String = conn
            .query_row("SELECT value FROM settings WHERE key = 'test'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(val, "hello");
    }

    #[test]
    fn concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let db = Arc::new(init_test_database());
        let mut handles = vec![];

        // Spawn 10 threads that each write and read settings
        for i in 0..10 {
            let db = Arc::clone(&db);
            handles.push(thread::spawn(move || {
                let key = format!("thread_{i}");
                let value = format!("value_{i}");
                db.set_setting(&key, &value).unwrap();
                let read = db.get_setting(&key);
                assert_eq!(read, Some(value));
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // All 10 settings should exist
        let all = db.get_all_settings();
        assert_eq!(all.len(), 10);
        for i in 0..10 {
            assert_eq!(all.get(&format!("thread_{i}")), Some(&format!("value_{i}")));
        }
    }

    #[test]
    fn concurrent_ui_state_access() {
        use std::sync::Arc;
        use std::thread;

        let db = Arc::new(init_test_database());
        let mut handles = vec![];

        // Multiple threads updating the same key
        for i in 0..20 {
            let db = Arc::clone(&db);
            handles.push(thread::spawn(move || {
                db.set_ui_state("counter", &i.to_string()).unwrap();
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // Key should exist with some value (last writer wins)
        let val = db.get_ui_state("counter");
        assert!(val.is_some());
        let num: i32 = val.unwrap().parse().unwrap();
        assert!((0..20).contains(&num));
    }

    // ── E2E Wiring Tests ──

    #[test]
    fn wiring_settings_roundtrip() {
        // Simulates: settings panel writes → SQLite → app startup reads
        let db = init_test_database();

        // 1. "Settings panel" saves values (what dbSetSetting does)
        db.set_setting("notification_sound_enabled", "true").unwrap();
        db.set_setting("ai_commit_message_enabled", "false").unwrap();
        db.set_setting("ai_resolver_strategy", "smart_merge").unwrap();
        db.set_setting("ai_commit_message_model", "claude-sonnet-4-20250514").unwrap();

        // 2. "App startup" loads all settings (what dbGetAllSettings does)
        let all = db.get_all_settings();

        // 3. Apply each setting to app state (what use-app-state-init.ts does)
        assert_eq!(all.get("notification_sound_enabled"), Some(&"true".into()));
        assert_eq!(all.get("ai_commit_message_enabled"), Some(&"false".into()));
        assert_eq!(all.get("ai_resolver_strategy"), Some(&"smart_merge".into()));
        assert_eq!(all.get("ai_commit_message_model"), Some(&"claude-sonnet-4-20250514".into()));

        // 4. Individual get also works (for targeted reads)
        assert_eq!(db.get_setting("notification_sound_enabled"), Some("true".into()));

        // 5. Settings not in DB return None → app uses defaults
        assert_eq!(db.get_setting("theme_preset"), None);
    }

    #[test]
    fn wiring_workspace_state_roundtrip() {
        // Simulates: activate_workspace writes → SQLite → app restart reads
        let db = init_test_database();

        // 1. "activate_workspace" saves active workspace ID
        db.set_ui_state("active_workspace", "workspace-abc123").unwrap();

        // 2. Also save collapse states for sidebar project groups
        db.set_ui_state("collapsed:project:/home/user/codemux", "true").unwrap();
        db.set_ui_state("collapsed:project:/home/user/other", "false").unwrap();

        // 3. Save window dimensions
        db.set_ui_state("window_width", "1920").unwrap();
        db.set_ui_state("window_height", "1080").unwrap();

        // 4. Save right panel width
        db.set_ui_state("right_panel_width", "320").unwrap();

        // 5. "App restart" reads everything back
        assert_eq!(db.get_ui_state("active_workspace"), Some("workspace-abc123".into()));
        assert_eq!(db.get_ui_state("collapsed:project:/home/user/codemux"), Some("true".into()));
        assert_eq!(db.get_ui_state("collapsed:project:/home/user/other"), Some("false".into()));
        assert_eq!(db.get_ui_state("window_width"), Some("1920".into()));
        assert_eq!(db.get_ui_state("window_height"), Some("1080".into()));
        assert_eq!(db.get_ui_state("right_panel_width"), Some("320".into()));

        // 6. Switch workspace → update
        db.set_ui_state("active_workspace", "workspace-def456").unwrap();
        assert_eq!(db.get_ui_state("active_workspace"), Some("workspace-def456".into()));
    }

    #[test]
    fn corruption_resilience() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("corrupt_test.db");

        // Phase 1: Create DB and write data
        {
            let conn = open_connection(&db_path).unwrap();
            create_schema(&conn).unwrap();
            let db = DatabaseStore { conn: Mutex::new(conn) };
            db.set_setting("important", "data").unwrap();
            db.set_ui_state("window_width", "1920").unwrap();
        }

        // Phase 2: Corrupt the file (truncate to 0 bytes)
        {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&db_path)
                .unwrap();
            file.write_all(b"").unwrap();
        }

        // Phase 3: Attempt to reopen — should not panic
        // SQLite will see it's not a valid DB and either:
        // a) Open successfully but fail on schema operations
        // b) Return an error on open
        // Either way, new_in_memory() fallback should work
        let result = open_connection(&db_path);
        match result {
            Ok(conn) => {
                // Connection opened but DB is empty/corrupt
                // create_schema may fail or succeed (SQLite may recreate from scratch)
                let schema_result = create_schema(&conn);
                if schema_result.is_ok() {
                    // Fresh DB was created — old data is gone but app works
                    let db = DatabaseStore { conn: Mutex::new(conn) };
                    assert_eq!(db.get_setting("important"), None); // Data lost, but no crash
                    db.set_setting("new_key", "works").unwrap(); // Can still write
                    assert_eq!(db.get_setting("new_key"), Some("works".into()));
                }
                // If schema_result failed, that's also acceptable — init_database
                // would fall back to new_in_memory() in the real app
            }
            Err(_) => {
                // Connection failed — the app's init_database fallback handles this
                let fallback = DatabaseStore::new_in_memory();
                fallback.set_setting("recovery", "works").unwrap();
                assert_eq!(fallback.get_setting("recovery"), Some("works".into()));
            }
        }
    }

    #[test]
    fn large_data_values() {
        let db = init_test_database();

        // 10KB setting value (like serialized layout state)
        let large_json = format!(
            r#"{{"workspaces":[{}]}}"#,
            (0..100)
                .map(|i| format!(r#"{{"id":"ws-{i}","title":"Workspace {i}","cwd":"/home/user/project-{i}","tabs":[{{"id":"tab-{i}","kind":"terminal"}}]}}"#))
                .collect::<Vec<_>>()
                .join(",")
        );
        assert!(large_json.len() > 10_000, "Test value should be >10KB, got {} bytes", large_json.len());

        db.set_setting("layout_state", &large_json).unwrap();
        let read_back = db.get_setting("layout_state");
        assert_eq!(read_back, Some(large_json.clone()));

        // 10KB ui_state value
        db.set_ui_state("scrollback_buffer", &large_json).unwrap();
        assert_eq!(db.get_ui_state("scrollback_buffer"), Some(large_json.clone()));

        // 50KB value (like OpenFlow comm log)
        let huge = "x".repeat(50_000);
        db.set_setting("comm_log", &huge).unwrap();
        assert_eq!(db.get_setting("comm_log"), Some(huge));

        // Multiple large values don't interfere
        let large_a = "a".repeat(10_000);
        let large_b = "b".repeat(10_000);
        db.set_setting("large_a", &large_a).unwrap();
        db.set_setting("large_b", &large_b).unwrap();
        assert_eq!(db.get_setting("large_a"), Some(large_a));
        assert_eq!(db.get_setting("large_b"), Some(large_b));
    }

    #[test]
    fn project_scripts_roundtrip() {
        let db = init_test_database();
        let scripts = ProjectScripts {
            setup: vec!["npm install".into(), "cp .env.example .env".into()],
            teardown: vec!["docker compose down".into()],
            run: Some("npm run dev".into()),
            ..Default::default()
        };
        db.set_project_scripts("/home/user/my-project", &scripts)
            .unwrap();
        let loaded = db
            .get_project_scripts("/home/user/my-project")
            .expect("scripts should exist");
        assert_eq!(loaded.setup, vec!["npm install", "cp .env.example .env"]);
        assert_eq!(loaded.teardown, vec!["docker compose down"]);
        assert_eq!(loaded.run, Some("npm run dev".into()));
    }

    #[test]
    fn project_scripts_missing_returns_none() {
        let db = init_test_database();
        assert!(db.get_project_scripts("/nonexistent").is_none());
    }

    #[test]
    fn project_scripts_update() {
        let db = init_test_database();
        let scripts = ProjectScripts {
            setup: vec!["npm install".into()],
            teardown: vec![],
            run: None,
            ..Default::default()
        };
        db.set_project_scripts("/project", &scripts).unwrap();

        let updated = ProjectScripts {
            setup: vec!["yarn install".into()],
            teardown: vec!["echo bye".into()],
            run: Some("yarn dev".into()),
            ..Default::default()
        };
        db.set_project_scripts("/project", &updated).unwrap();

        let loaded = db.get_project_scripts("/project").unwrap();
        assert_eq!(loaded.setup, vec!["yarn install"]);
        assert_eq!(loaded.teardown, vec!["echo bye"]);
        assert_eq!(loaded.run, Some("yarn dev".into()));
    }

    // ── Agent Chat Sessions ──

    #[test]
    fn agent_chat_sessions_upsert_and_fetch() {
        let db = init_test_database();
        db.upsert_agent_chat_session(
            "thread-1",
            "ws-1",
            Some("/tmp/proj"),
            "claude",
        )
        .unwrap();

        let rec = db
            .get_agent_chat_session("thread-1")
            .expect("row should exist");
        assert_eq!(rec.thread_id, "thread-1");
        assert_eq!(rec.workspace_id, "ws-1");
        assert_eq!(rec.cwd.as_deref(), Some("/tmp/proj"));
        assert_eq!(rec.provider, "claude");
        assert_eq!(rec.title, None);
        assert_eq!(rec.sdk_session_id, None);
    }

    #[test]
    fn agent_chat_sessions_upsert_on_conflict_preserves_identity_bumps_activity() {
        let db = init_test_database();
        db.upsert_agent_chat_session("t", "ws-1", Some("/a"), "claude")
            .unwrap();
        db.set_agent_chat_title("t", "Original title").unwrap();
        db.set_agent_chat_sdk_session_id("t", "sdk-uuid-xyz")
            .unwrap();

        // Re-upsert with a different provider — the row should
        // continue to exist with its identity fields intact.
        db.upsert_agent_chat_session("t", "ws-1", Some("/a"), "codex")
            .unwrap();

        let rec = db.get_agent_chat_session("t").unwrap();
        assert_eq!(rec.provider, "codex");
        assert_eq!(rec.title.as_deref(), Some("Original title"));
        assert_eq!(rec.sdk_session_id.as_deref(), Some("sdk-uuid-xyz"));
    }

    #[test]
    fn agent_chat_sessions_set_sdk_session_id() {
        let db = init_test_database();
        db.upsert_agent_chat_session("t", "ws-1", None, "claude")
            .unwrap();
        assert!(db.get_agent_chat_session("t").unwrap().sdk_session_id.is_none());

        db.set_agent_chat_sdk_session_id("t", "sdk-uuid-abc")
            .unwrap();
        assert_eq!(
            db.get_agent_chat_session("t")
                .unwrap()
                .sdk_session_id
                .as_deref(),
            Some("sdk-uuid-abc")
        );
    }

    #[test]
    fn agent_chat_sessions_title_roundtrip() {
        let db = init_test_database();
        db.upsert_agent_chat_session("t", "ws-1", None, "claude")
            .unwrap();
        assert_eq!(db.get_agent_chat_title("t"), None);
        db.set_agent_chat_title("t", "Refactor the auth layer")
            .unwrap();
        assert_eq!(
            db.get_agent_chat_title("t").as_deref(),
            Some("Refactor the auth layer")
        );
        // Updating overwrites.
        db.set_agent_chat_title("t", "Rename").unwrap();
        assert_eq!(db.get_agent_chat_title("t").as_deref(), Some("Rename"));
    }

    #[test]
    fn agent_chat_sessions_list_filters_by_workspace_and_cwd() {
        let db = init_test_database();
        db.upsert_agent_chat_session("t1", "ws-a", Some("/p1"), "claude")
            .unwrap();
        db.set_agent_chat_sdk_session_id("t1", "sdk-t1").unwrap();
        db.upsert_agent_chat_session("t2", "ws-a", Some("/p2"), "claude")
            .unwrap();
        db.set_agent_chat_sdk_session_id("t2", "sdk-t2").unwrap();
        db.upsert_agent_chat_session("t3", "ws-b", Some("/p1"), "claude")
            .unwrap();
        db.set_agent_chat_sdk_session_id("t3", "sdk-t3").unwrap();

        // Workspace-only scope returns every row in that workspace.
        let all_in_a = db.list_agent_chat_sessions("ws-a", None, 100);
        assert_eq!(all_in_a.len(), 2);

        // Workspace + cwd scope narrows further.
        let p1_in_a = db.list_agent_chat_sessions("ws-a", Some("/p1"), 100);
        assert_eq!(p1_in_a.len(), 1);
        assert_eq!(p1_in_a[0].thread_id, "t1");

        // Different workspace returns no crossover.
        let p1_in_b = db.list_agent_chat_sessions("ws-b", Some("/p1"), 100);
        assert_eq!(p1_in_b.len(), 1);
        assert_eq!(p1_in_b[0].thread_id, "t3");
    }

    #[test]
    fn agent_chat_sessions_list_hides_rows_without_sdk_session_id() {
        // Rows created by every start_session() but never graduated
        // to "have an SDK session id" are useless for resume — they
        // shouldn't appear in the dropdown. This is the main
        // practical cause of dropdown clutter: every silent restart
        // creates a fresh row and most never receive a
        // ResumeCursorUpdated event.
        let db = init_test_database();
        db.upsert_agent_chat_session("with-id", "ws", Some("/p"), "claude")
            .unwrap();
        db.set_agent_chat_sdk_session_id("with-id", "sdk-uuid").unwrap();
        db.upsert_agent_chat_session("no-id", "ws", Some("/p"), "claude")
            .unwrap();

        let listed = db.list_agent_chat_sessions("ws", None, 100);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].thread_id, "with-id");

        let scoped = db.list_agent_chat_sessions("ws", Some("/p"), 100);
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].thread_id, "with-id");
    }

    #[test]
    fn collapse_duplicate_agent_chat_sessions_keeps_survivor() {
        // Simulate the resume flow creating multiple rows for the
        // same logical chat: each row shares an sdk_session_id but
        // has a fresh thread_id.
        let db = init_test_database();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO agent_chat_sessions
                     (thread_id, sdk_session_id, workspace_id, provider, title, created_at, last_active_at)
                 VALUES ('oldest', 'sdk-xyz', 'ws', 'claude', 'Original', '2025-01-01', '2025-01-01')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO agent_chat_sessions
                     (thread_id, sdk_session_id, workspace_id, provider, created_at, last_active_at)
                 VALUES ('middle', 'sdk-xyz', 'ws', 'claude', '2025-02-01', '2025-02-01')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO agent_chat_sessions
                     (thread_id, sdk_session_id, workspace_id, provider, created_at, last_active_at)
                 VALUES ('newest', 'sdk-xyz', 'ws', 'claude', '2025-03-01', '2025-03-01')",
                [],
            )
            .unwrap();
        }

        db.collapse_duplicate_agent_chat_sessions("sdk-xyz")
            .unwrap();

        // Only the newest row survives.
        assert!(db.get_agent_chat_session("oldest").is_none());
        assert!(db.get_agent_chat_session("middle").is_none());
        let survivor = db.get_agent_chat_session("newest").unwrap();
        // Title carried forward from the original row.
        assert_eq!(survivor.title.as_deref(), Some("Original"));
        // created_at carried forward to the earliest value.
        assert_eq!(survivor.created_at, "2025-01-01");
    }

    #[test]
    fn collapse_duplicate_agent_chat_sessions_is_noop_when_no_match() {
        let db = init_test_database();
        db.upsert_agent_chat_session("t", "ws", None, "claude")
            .unwrap();
        // No sdk_session_id set — collapse must not touch the row.
        db.collapse_duplicate_agent_chat_sessions("sdk-missing")
            .unwrap();
        assert!(db.get_agent_chat_session("t").is_some());
    }

    #[test]
    fn agent_chat_sessions_list_orders_by_last_active_desc() {
        let db = init_test_database();
        // Seed with explicit timestamps so ordering is deterministic.
        // Each row needs a non-null sdk_session_id or list() filters
        // it out.
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO agent_chat_sessions
                     (thread_id, sdk_session_id, workspace_id, provider, created_at, last_active_at)
                 VALUES ('old', 'sdk-old', 'ws', 'claude', '2025-01-01', '2025-01-01')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO agent_chat_sessions
                     (thread_id, sdk_session_id, workspace_id, provider, created_at, last_active_at)
                 VALUES ('newest', 'sdk-newest', 'ws', 'claude', '2025-01-01', '2025-06-01')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO agent_chat_sessions
                     (thread_id, sdk_session_id, workspace_id, provider, created_at, last_active_at)
                 VALUES ('mid', 'sdk-mid', 'ws', 'claude', '2025-01-01', '2025-03-01')",
                [],
            )
            .unwrap();
        }
        let rows = db.list_agent_chat_sessions("ws", None, 10);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].thread_id, "newest");
        assert_eq!(rows[1].thread_id, "mid");
        assert_eq!(rows[2].thread_id, "old");
    }

    #[test]
    fn agent_chat_sessions_touch_floats_to_top() {
        let db = init_test_database();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO agent_chat_sessions
                     (thread_id, sdk_session_id, workspace_id, provider, created_at, last_active_at)
                 VALUES ('a', 'sdk-a', 'ws', 'claude', '2025-01-01', '2025-01-01')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO agent_chat_sessions
                     (thread_id, sdk_session_id, workspace_id, provider, created_at, last_active_at)
                 VALUES ('b', 'sdk-b', 'ws', 'claude', '2025-01-01', '2025-06-01')",
                [],
            )
            .unwrap();
        }
        // Before touch, 'b' is on top.
        let before = db.list_agent_chat_sessions("ws", None, 10);
        assert_eq!(before[0].thread_id, "b");

        // After touch on 'a', 'a' has today's timestamp → top.
        db.touch_agent_chat_session("a").unwrap();
        let after = db.list_agent_chat_sessions("ws", None, 10);
        assert_eq!(after[0].thread_id, "a");
    }

    #[test]
    fn agent_chat_sessions_delete_is_idempotent() {
        let db = init_test_database();
        db.upsert_agent_chat_session("t", "ws", None, "claude")
            .unwrap();
        assert!(db.get_agent_chat_session("t").is_some());
        db.delete_agent_chat_session("t").unwrap();
        assert!(db.get_agent_chat_session("t").is_none());
        // Second call is a no-op.
        db.delete_agent_chat_session("t").unwrap();
    }

    #[test]
    fn agent_chat_sessions_migrate_carries_forward_identity() {
        let db = init_test_database();
        db.upsert_agent_chat_session("old", "ws", Some("/p"), "claude")
            .unwrap();
        db.set_agent_chat_title("old", "My chat").unwrap();
        db.set_agent_chat_sdk_session_id("old", "sdk-uuid").unwrap();

        // Simulate migrateThreadId: caller upserts the new row first,
        // then asks us to carry forward the identity fields.
        db.upsert_agent_chat_session("new", "ws", Some("/p"), "claude")
            .unwrap();
        db.migrate_agent_chat_session("old", "new").unwrap();

        assert!(db.get_agent_chat_session("old").is_none());
        let rec = db.get_agent_chat_session("new").unwrap();
        assert_eq!(rec.title.as_deref(), Some("My chat"));
        assert_eq!(rec.sdk_session_id.as_deref(), Some("sdk-uuid"));
    }

    #[test]
    fn agent_chat_sessions_limit_caps_rows() {
        let db = init_test_database();
        for i in 0..10 {
            db.upsert_agent_chat_session(
                &format!("t{i}"),
                "ws",
                None,
                "claude",
            )
            .unwrap();
            db.set_agent_chat_sdk_session_id(&format!("t{i}"), &format!("sdk-{i}"))
                .unwrap();
        }
        let capped = db.list_agent_chat_sessions("ws", None, 3);
        assert_eq!(capped.len(), 3);
    }

    // ── Agent Chat Messages ──

    fn seed_session(db: &DatabaseStore, thread_id: &str) {
        db.upsert_agent_chat_session(thread_id, "ws", Some("/p"), "claude")
            .unwrap();
    }

    #[test]
    fn agent_chat_messages_append_and_list_preserves_insertion_order() {
        let db = init_test_database();
        seed_session(&db, "t");

        for i in 0..5 {
            db.append_agent_chat_message("t", &format!(r#"{{"i":{i}}}"#))
                .unwrap();
        }

        let rows = db.list_agent_chat_messages("t");
        assert_eq!(rows.len(), 5);
        for (i, row) in rows.iter().enumerate() {
            assert_eq!(row, &format!(r#"{{"i":{i}}}"#));
        }
    }

    #[test]
    fn agent_chat_messages_list_returns_empty_for_unknown_thread() {
        let db = init_test_database();
        assert!(db.list_agent_chat_messages("nope").is_empty());
    }

    #[test]
    fn agent_chat_messages_list_filters_by_thread() {
        let db = init_test_database();
        seed_session(&db, "t1");
        seed_session(&db, "t2");

        db.append_agent_chat_message("t1", r#"{"a":1}"#).unwrap();
        db.append_agent_chat_message("t2", r#"{"b":2}"#).unwrap();
        db.append_agent_chat_message("t1", r#"{"c":3}"#).unwrap();

        let t1 = db.list_agent_chat_messages("t1");
        assert_eq!(t1, vec![r#"{"a":1}"#.to_string(), r#"{"c":3}"#.to_string()]);
        let t2 = db.list_agent_chat_messages("t2");
        assert_eq!(t2, vec![r#"{"b":2}"#.to_string()]);
    }

    #[test]
    fn agent_chat_messages_cascade_delete_on_session_drop() {
        // Cleanup hygiene: deleting a session row from the dropdown
        // also drops its persisted transcript, freeing disk and
        // hiding the chat from any "leak" of orphan rows.
        let db = init_test_database();
        seed_session(&db, "t");
        db.append_agent_chat_message("t", r#"{"x":1}"#).unwrap();
        db.append_agent_chat_message("t", r#"{"x":2}"#).unwrap();
        assert_eq!(db.list_agent_chat_messages("t").len(), 2);

        db.delete_agent_chat_session("t").unwrap();
        assert!(db.list_agent_chat_messages("t").is_empty());
    }

    #[test]
    fn agent_chat_messages_append_to_unknown_thread_is_silent_noop() {
        // FK violation: parent session row absent. The contract is
        // "best-effort persistence" — the event-bridge loop must not
        // crash on a race where forward_event fires after the user
        // has deleted the session.
        let db = init_test_database();
        let result = db.append_agent_chat_message("ghost", r#"{"x":1}"#);
        assert!(
            result.is_ok(),
            "FK violation should be swallowed silently"
        );
        assert!(db.list_agent_chat_messages("ghost").is_empty());
    }

    #[test]
    fn agent_chat_messages_append_surfaces_unrelated_errors() {
        // Closed-on-purpose: only FK_CONSTRAINT is swallowed. Other
        // SQL errors must propagate so we don't silently accumulate
        // data corruption. Without an easy way to force a non-FK
        // failure in-memory, this test acts as documentation: if this
        // assert ever changes shape, revisit `append_agent_chat_message`.
        let db = init_test_database();
        seed_session(&db, "t");
        // Sanity: the happy path still returns Ok on a real row.
        assert!(db
            .append_agent_chat_message("t", r#"{"ok":true}"#)
            .is_ok());
    }

    #[test]
    fn migrate_agent_chat_session_moves_messages_to_new_thread() {
        // Silent restart path: the user changes permission_mode mid-
        // conversation. The store-side migrateThreadId carries the
        // transcript forward in memory; the DB-side migration must
        // do the same, otherwise the next list_messages call sees an
        // empty history under the new thread id.
        let db = init_test_database();
        seed_session(&db, "old");
        db.append_agent_chat_message("old", r#"{"i":1}"#).unwrap();
        db.append_agent_chat_message("old", r#"{"i":2}"#).unwrap();

        seed_session(&db, "new");
        db.migrate_agent_chat_session("old", "new").unwrap();

        // Messages now live under "new", and the source row is gone.
        assert!(db.list_agent_chat_messages("old").is_empty());
        let migrated = db.list_agent_chat_messages("new");
        assert_eq!(migrated, vec![r#"{"i":1}"#.to_string(), r#"{"i":2}"#.to_string()]);
        assert!(db.get_agent_chat_session("old").is_none());
    }

    #[test]
    fn collapse_duplicate_agent_chat_sessions_moves_messages_onto_survivor() {
        // The dropdown's "Resume" path produces a new thread_id
        // sharing the original's sdk_session_id. Without the
        // pre-collapse UPDATE on agent_chat_messages, the FK cascade
        // would wipe the historical transcript when the duplicate
        // session row gets dropped — exactly the regression the
        // resume feature is meant to avoid.
        let db = init_test_database();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO agent_chat_sessions
                     (thread_id, sdk_session_id, workspace_id, provider, created_at, last_active_at)
                 VALUES ('oldest', 'sdk-xyz', 'ws', 'claude', '2025-01-01', '2025-01-01')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO agent_chat_sessions
                     (thread_id, sdk_session_id, workspace_id, provider, created_at, last_active_at)
                 VALUES ('newest', 'sdk-xyz', 'ws', 'claude', '2025-03-01', '2025-03-01')",
                [],
            )
            .unwrap();
        }
        db.append_agent_chat_message("oldest", r#"{"i":"old1"}"#)
            .unwrap();
        db.append_agent_chat_message("oldest", r#"{"i":"old2"}"#)
            .unwrap();
        db.append_agent_chat_message("newest", r#"{"i":"new1"}"#)
            .unwrap();

        db.collapse_duplicate_agent_chat_sessions("sdk-xyz").unwrap();

        // Survivor "newest" now owns every message; the merged-out
        // row is gone.
        assert!(db.get_agent_chat_session("oldest").is_none());
        assert!(db.list_agent_chat_messages("oldest").is_empty());
        let survivor = db.list_agent_chat_messages("newest");
        // Order across the moved-in rows preserves the insertion id;
        // we don't test exact ordering here (it can interleave by id)
        // but verify count + every payload is present.
        assert_eq!(survivor.len(), 3);
        let joined = survivor.join(",");
        assert!(joined.contains(r#"{"i":"old1"}"#));
        assert!(joined.contains(r#"{"i":"old2"}"#));
        assert!(joined.contains(r#"{"i":"new1"}"#));
    }

    #[test]
    fn agent_chat_messages_supports_large_payloads() {
        // Tool results can carry serialized file contents — ~50KB
        // is plausible. SQLite handles arbitrary TEXT lengths but
        // make sure no hidden truncation crept in via column types.
        let db = init_test_database();
        seed_session(&db, "t");
        let big = format!(r#"{{"data":"{}"}}"#, "x".repeat(50_000));
        db.append_agent_chat_message("t", &big).unwrap();
        let rows = db.list_agent_chat_messages("t");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], big);
    }

    #[test]
    fn agent_chat_messages_high_volume_round_trip() {
        // Sanity: a long conversation (5 turns × ~10 events per
        // turn) round-trips cleanly. Catches any per-row overhead
        // regressions or query-prepare bugs that might surface only
        // at scale.
        let db = init_test_database();
        seed_session(&db, "t");
        for i in 0..500 {
            db.append_agent_chat_message("t", &format!(r#"{{"i":{i}}}"#))
                .unwrap();
        }
        let rows = db.list_agent_chat_messages("t");
        assert_eq!(rows.len(), 500);
        // Ordering is monotonic by insertion.
        for (i, row) in rows.iter().enumerate() {
            assert_eq!(row, &format!(r#"{{"i":{i}}}"#));
        }
    }

    #[test]
    fn agent_chat_messages_delete_for_thread_is_idempotent() {
        let db = init_test_database();
        seed_session(&db, "t");
        db.append_agent_chat_message("t", r#"{"i":1}"#).unwrap();
        db.delete_agent_chat_messages_for_thread("t").unwrap();
        assert!(db.list_agent_chat_messages("t").is_empty());
        // Second call on already-empty thread doesn't error.
        db.delete_agent_chat_messages_for_thread("t").unwrap();
        // Session row is untouched by message-only deletion.
        assert!(db.get_agent_chat_session("t").is_some());
    }

    #[test]
    fn agent_chat_messages_lifecycle_survives_reopen() {
        // End-to-end persistence guarantee: messages written to a
        // file-backed DB are still there after the process restarts.
        // Closes the loop — without this, replay-on-resume would only
        // work within a single session.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.db");

        {
            let conn = open_connection(&path).unwrap();
            create_schema(&conn).unwrap();
            let db = DatabaseStore { conn: Mutex::new(conn) };
            db.upsert_agent_chat_session("t", "ws", Some("/p"), "claude")
                .unwrap();
            db.append_agent_chat_message("t", r#"{"i":1}"#).unwrap();
            db.append_agent_chat_message("t", r#"{"i":2}"#).unwrap();
        }

        let conn = open_connection(&path).unwrap();
        create_schema(&conn).unwrap();
        let db = DatabaseStore { conn: Mutex::new(conn) };
        let rows = db.list_agent_chat_messages("t");
        assert_eq!(rows, vec![r#"{"i":1}"#.to_string(), r#"{"i":2}"#.to_string()]);
    }

    #[test]
    fn agent_chat_sessions_lifecycle_survives_reopen() {
        // Confirms the migration upgrades a legacy v1 DB cleanly:
        // schema_version starts at 2, and sessions round-trip.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.db");

        {
            let conn = open_connection(&path).unwrap();
            create_schema(&conn).unwrap();
            let db = DatabaseStore { conn: Mutex::new(conn) };
            db.upsert_agent_chat_session("t", "ws", Some("/p"), "claude")
                .unwrap();
            db.set_agent_chat_title("t", "Persisted").unwrap();
        }

        let conn = open_connection(&path).unwrap();
        create_schema(&conn).unwrap();
        let db = DatabaseStore { conn: Mutex::new(conn) };
        let version: u32 = db
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let rec = db.get_agent_chat_session("t").unwrap();
        assert_eq!(rec.title.as_deref(), Some("Persisted"));
    }

    // ── Hosts CRUD tests ──
    //
    // These exercise the soft-delete + dirty-flag invariants the sync
    // layer relies on. A bug here means hosts silently disappear or
    // duplicate on the user's other devices — much worse than a UI
    // glitch, so the coverage is intentionally thorough.

    #[test]
    fn hosts_insert_and_list() {
        let db = init_test_database();
        assert!(db.list_hosts().is_empty());

        let h = db.insert_host("homelab", "zeus@10.0.0.5").unwrap();
        assert_eq!(h.name, "homelab");
        assert_eq!(h.ssh_target, "zeus@10.0.0.5");
        assert!(h.dirty, "new rows must be marked dirty so sync picks them up");
        assert!(h.server_id.is_none(), "fresh inserts have no server_id");
        assert!(h.deleted_at.is_none());

        let list = db.list_hosts();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, h.id);
    }

    #[test]
    fn hosts_list_ordered_case_insensitive() {
        let db = init_test_database();
        db.insert_host("zebra", "u@a").unwrap();
        db.insert_host("Apple", "u@b").unwrap();
        db.insert_host("banana", "u@c").unwrap();
        let names: Vec<String> = db.list_hosts().into_iter().map(|h| h.name).collect();
        assert_eq!(names, vec!["Apple", "banana", "zebra"]);
    }

    #[test]
    fn hosts_update_marks_dirty() {
        let db = init_test_database();
        let h = db.insert_host("orig", "old@host").unwrap();
        db.mark_host_synced(h.id, Some("srv-1")).unwrap();
        // After mark_synced, the row should be clean.
        let clean = db.list_hosts().into_iter().find(|x| x.id == h.id).unwrap();
        assert!(!clean.dirty);
        assert_eq!(clean.server_id.as_deref(), Some("srv-1"));

        let updated = db.update_host(h.id, "renamed", "new@host").unwrap();
        assert_eq!(updated.name, "renamed");
        assert_eq!(updated.ssh_target, "new@host");
        assert!(updated.dirty, "edits must re-mark the row dirty");
        assert_eq!(
            updated.server_id.as_deref(),
            Some("srv-1"),
            "server_id survives a rename so we update-not-recreate on push"
        );
    }

    #[test]
    fn hosts_update_unknown_id_errors() {
        let db = init_test_database();
        let result = db.update_host(9999, "x", "y");
        assert!(result.is_err());
    }

    #[test]
    fn hosts_delete_is_soft_and_dirty() {
        let db = init_test_database();
        let h = db.insert_host("doomed", "u@h").unwrap();
        db.delete_host(h.id).unwrap();

        // Soft-deleted rows do NOT appear in list_hosts.
        assert!(db.list_hosts().is_empty());

        // But they DO appear in list_hosts_for_sync so the tombstone
        // can be pushed to the server.
        let pending = db.list_hosts_for_sync();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].deleted_at.is_some());
        assert!(
            pending[0].dirty,
            "tombstones must be dirty so the sync layer pushes them"
        );
    }

    #[test]
    fn hosts_delete_unknown_id_errors() {
        let db = init_test_database();
        assert!(db.delete_host(9999).is_err());
    }

    #[test]
    fn hosts_dirty_list_filters_correctly() {
        let db = init_test_database();
        let dirty = db.insert_host("a", "u@a").unwrap();
        let clean = db.insert_host("b", "u@b").unwrap();
        db.mark_host_synced(clean.id, Some("srv-b")).unwrap();

        let only_dirty = db.list_dirty_hosts();
        assert_eq!(only_dirty.len(), 1);
        assert_eq!(only_dirty[0].id, dirty.id);
    }

    #[test]
    fn hosts_purge_acknowledged_deletes() {
        let db = init_test_database();
        let h = db.insert_host("temp", "u@t").unwrap();
        db.delete_host(h.id).unwrap();
        // Before mark_synced: still a tombstone, must NOT be purged.
        db.purge_acknowledged_deletes().unwrap();
        assert_eq!(db.list_hosts_for_sync().len(), 1);
        // After mark_synced: tombstone is acknowledged, NOW purge.
        db.mark_host_synced(h.id, Some("srv-t")).unwrap();
        db.purge_acknowledged_deletes().unwrap();
        assert!(db.list_hosts_for_sync().is_empty());
    }

    #[test]
    fn hosts_upsert_from_server_new_then_update() {
        let db = init_test_database();
        // First sync: server has a row we don't.
        db.upsert_host_from_server(
            "srv-1",
            "from-cloud",
            "user@cloud",
            "2026-05-01 12:00:00",
            "2026-05-01 12:00:00",
            None,
        )
        .unwrap();
        let after_first = db.list_hosts();
        assert_eq!(after_first.len(), 1);
        assert_eq!(after_first[0].server_id.as_deref(), Some("srv-1"));
        assert!(
            !after_first[0].dirty,
            "server-sourced rows must NOT be dirty (they already match the server)"
        );

        // Second sync: server reports a rename. We must update in place,
        // not insert a duplicate.
        db.upsert_host_from_server(
            "srv-1",
            "renamed-from-cloud",
            "user@cloud",
            "2026-05-01 12:00:00",
            "2026-05-02 09:00:00",
            None,
        )
        .unwrap();
        let after_second = db.list_hosts();
        assert_eq!(after_second.len(), 1, "no duplicate row");
        assert_eq!(after_second[0].name, "renamed-from-cloud");

        // Third sync: server marks the row deleted.
        db.upsert_host_from_server(
            "srv-1",
            "renamed-from-cloud",
            "user@cloud",
            "2026-05-01 12:00:00",
            "2026-05-03 09:00:00",
            Some("2026-05-03 09:00:00"),
        )
        .unwrap();
        // list_hosts hides deleted rows; list_hosts_for_sync sees them.
        assert!(db.list_hosts().is_empty());
        let raw = db.list_hosts_for_sync();
        assert_eq!(raw.len(), 1);
        assert!(raw[0].deleted_at.is_some());
    }

    #[test]
    fn hosts_local_and_remote_coexist_until_paired() {
        // Realistic scenario: user adds a host on their laptop while
        // offline. Meanwhile their desktop synced a different host.
        // Once auth comes back and pull/push run, both rows should
        // coexist with distinct server_ids — no merge collision.
        let db = init_test_database();
        let local = db.insert_host("laptop-only", "u@laptop").unwrap();
        db.upsert_host_from_server(
            "srv-desktop",
            "desktop-only",
            "u@desktop",
            "2026-05-01 12:00:00",
            "2026-05-01 12:00:00",
            None,
        )
        .unwrap();
        let list = db.list_hosts();
        assert_eq!(list.len(), 2);
        // Pretend the local row got pushed; mark it synced.
        db.mark_host_synced(local.id, Some("srv-laptop")).unwrap();
        // Now both rows have distinct server_ids.
        let mut sids: Vec<String> = db
            .list_hosts()
            .into_iter()
            .filter_map(|h| h.server_id)
            .collect();
        sids.sort();
        assert_eq!(sids, vec!["srv-desktop".to_string(), "srv-laptop".to_string()]);
    }
}
