//! Hermes identity and cleanup holds deliberately outlive local chat deletion.
use super::*;
use crate::agent_provider::hermes::binding::Binding;

pub(super) fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS hermes_bindings (
        thread_id TEXT PRIMARY KEY, workspace_id TEXT, cwd TEXT NOT NULL,
        binding TEXT NOT NULL, cleanup_pending INTEGER NOT NULL DEFAULT 1
    );",
    )
}

impl DatabaseStore {
    /// Archive restore creates a new workspace identity. Reattach only local
    /// history at the exact retained cwd; native identity/cleanup holds stay intact.
    pub fn restore_hermes_chat_history(
        &self,
        old: &str,
        restored: &str,
        cwd: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("UPDATE agent_chat_sessions SET workspace_id=?2 WHERE workspace_id=?1 AND cwd=?3 AND provider='hermes'", params![old, restored, cwd]).map_err(|e| e.to_string())?;
        Ok(())
    }
    /// Commit UI intent and the provider binding together, including offline changes.
    /// Effective model remains separate until a native load verifies it.
    pub fn update_hermes_intent(
        &self,
        thread: &str,
        model: Option<&str>,
        mode: Option<&str>,
    ) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let raw: String = tx
            .query_row(
                "SELECT binding FROM hermes_bindings WHERE thread_id=?1",
                [thread],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let mut binding: Binding = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        if let Some(model) = model {
            binding.model_override = (model != "profile_default").then(|| model.to_string());
        }
        if let Some(mode) = mode {
            binding.permission_mode = Some(mode.to_string());
        }
        tx.execute("UPDATE agent_chat_sessions SET model=COALESCE(?2,model), permission_mode=COALESCE(?3,permission_mode) WHERE thread_id=?1", params![thread,model,mode]).map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE hermes_bindings SET binding=?2 WHERE thread_id=?1",
            params![
                thread,
                serde_json::to_string(&binding).map_err(|e| e.to_string())?
            ],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }

    pub fn hermes_workspace_threads(&self, workspace: &str) -> Result<Vec<String>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT thread_id FROM hermes_bindings WHERE workspace_id=?1")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([workspace], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    pub fn hermes_binding(&self, thread: &str) -> Result<Option<Binding>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let value: Option<String> = conn
            .query_row(
                "SELECT binding FROM hermes_bindings WHERE thread_id=?1",
                [thread],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        value
            .map(|s| {
                serde_json::from_str(&s)
                    .map_err(|e| format!("repair_required: invalid Hermes binding: {e}"))
            })
            .transpose()
    }

    pub fn save_hermes_binding(&self, binding: &Binding) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let serialized = serde_json::to_string(binding).map_err(|e| e.to_string())?;
        conn.execute("INSERT INTO hermes_bindings(thread_id,workspace_id,cwd,binding,cleanup_pending) VALUES (?1,?2,?3,?4,?5)
            ON CONFLICT(thread_id) DO UPDATE SET binding=excluded.binding, cleanup_pending=excluded.cleanup_pending",
            params![binding.thread_id, binding.workspace_id, binding.cwd.to_string_lossy(), serialized, binding.cleanup_pending]).map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn hermes_cleanup_pending_path(&self, cwd: &str) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let path = std::path::Path::new(cwd)
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(cwd));
        let mut stmt = conn
            .prepare("SELECT cwd FROM hermes_bindings WHERE cleanup_pending=1")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        for row in rows {
            if std::path::Path::new(&row.map_err(|e| e.to_string())?).starts_with(&path) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn hermes_cleanup_pending(&self, workspace: &str) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row("SELECT EXISTS(SELECT 1 FROM hermes_bindings WHERE workspace_id=?1 AND cleanup_pending=1)", [workspace], |r| r.get(0)).map_err(|e| e.to_string())
    }
}
