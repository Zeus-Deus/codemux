//! Local journal for terminal closes pending daemon acknowledgment or layout persistence.

use super::DatabaseStore;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct DaemonWorkspaceRoute {
    pub(crate) workspace_id: String,
    pub(crate) host_id: Option<i64>,
    pub(crate) ssh_target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingDaemonClose {
    pub(crate) route: DaemonWorkspaceRoute,
    pub(crate) session_id: String,
    // A nil UUID records a layout removal with no shell left to close.
    pub(crate) shell_id: Uuid,
}

pub(super) fn create_schema(conn: &Connection) -> Result<(), String> {
    // The shell can outlive both the layout workspace and its saved host.
    // The captured route is part of its identity and has no foreign key.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS pending_daemon_closes (
            route TEXT NOT NULL,
            session_id TEXT NOT NULL,
            shell_id BLOB NOT NULL CHECK (length(shell_id) = 16),
            PRIMARY KEY (route, session_id, shell_id)
        );",
    )
    .map_err(|error| format!("Failed to initialize terminal close journal: {error}"))
}

impl DatabaseStore {
    pub(crate) fn record_pending_daemon_closes(
        &self,
        route: &DaemonWorkspaceRoute,
        session_id: &str,
        shell_ids: &[Uuid],
    ) -> Result<(), String> {
        if shell_ids.is_empty() {
            return Ok(());
        }
        let route = serde_json::to_string(route).map_err(|error| error.to_string())?;
        let mut conn = self.conn.lock().unwrap();
        if conn.path().is_none_or(str::is_empty) {
            return Err("Terminal close journal has no on-disk database".into());
        }
        let tx = conn.transaction().map_err(|error| error.to_string())?;
        {
            let mut insert = tx
                .prepare_cached(
                    "INSERT INTO pending_daemon_closes (route, session_id, shell_id)
                 VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING",
                )
                .map_err(|error| error.to_string())?;
            for shell_id in shell_ids {
                insert
                    .execute(params![route, session_id, shell_id.as_bytes().as_slice()])
                    .map_err(|error| error.to_string())?;
            }
        }
        tx.commit().map_err(|error| error.to_string())
    }

    pub(crate) fn pending_daemon_closes(&self) -> Result<Vec<PendingDaemonClose>, String> {
        let conn = self.conn.lock().unwrap();
        let mut select = conn
            .prepare("SELECT route, session_id, shell_id FROM pending_daemon_closes ORDER BY rowid")
            .map_err(|error| error.to_string())?;
        let rows = select
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(|error| error.to_string())?;
        rows.map(|row| {
            let (route, session_id, shell_id) = row.map_err(|error| error.to_string())?;
            let route = serde_json::from_str(&route).map_err(|error| error.to_string())?;
            Ok(PendingDaemonClose {
                route,
                session_id,
                shell_id: Uuid::from_bytes(shell_id),
            })
        })
        .collect()
    }

    pub(crate) fn retire_completed_daemon_closes(
        &self,
        route: &DaemonWorkspaceRoute,
        session_id: &str,
        shell_ids: &[Uuid],
    ) -> Result<(), String> {
        let route = serde_json::to_string(route).map_err(|error| error.to_string())?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|error| error.to_string())?;
        {
            let mut delete = tx.prepare_cached(
                "DELETE FROM pending_daemon_closes WHERE route = ?1 AND session_id = ?2 AND shell_id = ?3",
            ).map_err(|error| error.to_string())?;
            for shell_id in shell_ids {
                delete
                    .execute(params![route, session_id, shell_id.as_bytes().as_slice()])
                    .map_err(|error| error.to_string())?;
            }
        }
        tx.commit().map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_close_journal_reopens_and_acknowledges_only_the_exact_route_and_shell() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.sqlite");
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let route = DaemonWorkspaceRoute {
            workspace_id: "workspace".into(),
            host_id: Some(1),
            ssh_target: Some("original-host".into()),
        };
        let other_route = DaemonWorkspaceRoute {
            host_id: Some(2),
            ssh_target: Some("different-host".into()),
            ..route.clone()
        };

        {
            let db = DatabaseStore::test_file(&path);
            db.record_pending_daemon_closes(&route, "session", &[first, second, first])
                .unwrap();
            db.record_pending_daemon_closes(&other_route, "session", &[first])
                .unwrap();
        }

        {
            let db = DatabaseStore::test_file(&path);
            let jobs = db.pending_daemon_closes().unwrap();
            assert_eq!(jobs.len(), 3);
            assert_eq!(jobs[0].route, route);
            assert_eq!(jobs[0].shell_id, first);
            db.retire_completed_daemon_closes(&route, "session", &[first])
                .unwrap();
        }

        let db = DatabaseStore::test_file(&path);
        let jobs = db.pending_daemon_closes().unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].route, route);
        assert_eq!(jobs[0].shell_id, second);
        assert_eq!(jobs[1].route, other_route);
        assert_eq!(jobs[1].shell_id, first);
    }
}
