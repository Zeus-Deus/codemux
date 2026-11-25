use super::DatabaseStore;
use crate::config::workspace_config::LifecycleWorktreeId;
use rusqlite::{params, OptionalExtension};

pub(super) const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS lifecycle_worktrees (
        workspace_id TEXT PRIMARY KEY,
        directory_ids TEXT NOT NULL
    );
";

impl DatabaseStore {
    pub(crate) fn lifecycle_worktree_id(
        &self,
        workspace_id: &str,
    ) -> Result<Option<LifecycleWorktreeId>, String> {
        let conn = self.conn.lock().map_err(|error| error.to_string())?;
        let saved: Option<String> = conn
            .query_row(
                "SELECT directory_ids FROM lifecycle_worktrees WHERE workspace_id = ?1",
                [workspace_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        saved
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|error| error.to_string())
    }

    pub(crate) fn bind_lifecycle_worktree(
        &self,
        workspace_id: &str,
        identity: &LifecycleWorktreeId,
    ) -> Result<(), String> {
        let value = serde_json::to_string(identity).map_err(|error| error.to_string())?;
        let conn = self.conn.lock().map_err(|error| error.to_string())?;
        conn.execute(
            "INSERT INTO lifecycle_worktrees (workspace_id, directory_ids) VALUES (?1, ?2)
             ON CONFLICT(workspace_id) DO NOTHING",
            params![workspace_id, value],
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    }
}
