use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

const SCHEMA_VERSION: u32 = 1;

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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectScripts {
    #[serde(default)]
    pub setup: Vec<String>,
    #[serde(default)]
    pub teardown: Vec<String>,
    #[serde(default)]
    pub run: Option<String>,
}

fn database_path() -> Option<PathBuf> {
    let config = dirs::config_dir()?;
    Some(config.join("codemux").join("codemux.db"))
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
        ",
    )
    .map_err(|e| format!("Failed to create database schema: {e}"))?;

    // Set schema version if not already set
    let count: i32 = conn
        .query_row("SELECT COUNT(*) FROM schema_version", [], |row| row.get(0))
        .map_err(|e| format!("Failed to check schema version: {e}"))?;

    if count == 0 {
        conn.execute(
            "INSERT INTO schema_version (version) VALUES (?1)",
            params![SCHEMA_VERSION],
        )
        .map_err(|e| format!("Failed to set schema version: {e}"))?;
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
        };
        db.set_project_scripts("/project", &scripts).unwrap();

        let updated = ProjectScripts {
            setup: vec!["yarn install".into()],
            teardown: vec!["echo bye".into()],
            run: Some("yarn dev".into()),
        };
        db.set_project_scripts("/project", &updated).unwrap();

        let loaded = db.get_project_scripts("/project").unwrap();
        assert_eq!(loaded.setup, vec!["yarn install"]);
        assert_eq!(loaded.teardown, vec!["echo bye"]);
        assert_eq!(loaded.run, Some("yarn dev".into()));
    }
}
