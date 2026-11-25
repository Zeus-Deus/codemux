//! The user's approvals of MCP servers a project's own files define (see
//! `mcp::project_trust`). Device-local and never synced. Kept out of the
//! generic settings table so paired devices, which may read settings,
//! never see the fingerprints or project paths.
use super::*;

pub(super) fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS mcp_project_approvals (
            fingerprint TEXT PRIMARY KEY, project_root TEXT NOT NULL,
            name TEXT NOT NULL, approved_at INTEGER NOT NULL
        );",
    )
}

impl DatabaseStore {
    pub fn mcp_project_server_approved(&self, fingerprint: &str) -> bool {
        let Ok(conn) = self.conn.lock() else { return false };
        conn.query_row(
            "SELECT 1 FROM mcp_project_approvals WHERE fingerprint=?1",
            [fingerprint],
            |_| Ok(()),
        )
        .is_ok()
    }

    pub fn approve_mcp_project_server(
        &self,
        fingerprint: &str,
        project_root: &str,
        name: &str,
    ) -> Result<(), String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis() as i64);
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR REPLACE INTO mcp_project_approvals (fingerprint, project_root, name, approved_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![fingerprint, project_root, name, now],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Forget an approval. Returns whether there was one.
    pub fn revoke_mcp_project_approval(&self, fingerprint: &str) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM mcp_project_approvals WHERE fingerprint=?1", [fingerprint])
            .map(|removed| removed > 0)
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approvals_are_recorded_and_revoked_outside_settings() {
        let db = DatabaseStore::new_in_memory();
        assert!(!db.mcp_project_server_approved("fp"));
        db.approve_mcp_project_server("fp", "/repo", "lint").unwrap();
        assert!(db.mcp_project_server_approved("fp"));
        assert!(!db.get_all_settings().iter().any(|(k, v)| k.contains("fp") || v.contains("/repo")));
        assert!(db.revoke_mcp_project_approval("fp").unwrap());
        assert!(!db.mcp_project_server_approved("fp"));
        assert!(!db.revoke_mcp_project_approval("fp").unwrap());
    }
}
