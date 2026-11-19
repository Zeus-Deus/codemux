//! Local journal for layout closes and their independent guarded daemon cleanup.

use super::DatabaseStore;
use crate::state::{ArchivedWorkspaceSnapshot, LayoutCloseIntent};
use rusqlite::{params, Connection, Transaction, TransactionBehavior};
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
    pub(crate) shell_id: Uuid,
}

impl LayoutCloseIntent {
    fn row_key(&self) -> (&str, &str, &str) {
        match self {
            Self::Pane(id) => ("pane", "", id),
            Self::Session {
                workspace_id,
                session_id,
            } => ("session", workspace_id, session_id),
            Self::Tab {
                workspace_id,
                tab_id,
            } => ("tab", workspace_id, tab_id),
            Self::Workspace(id) => ("workspace", id, ""),
            Self::Archive(entry) => ("archive", &entry.workspace_id, &entry.archive_id),
        }
    }
}

pub(super) fn create_schema(conn: &Connection) -> Result<(), String> {
    migrate_schema(conn)
        .map_err(|error| format!("Failed to initialize terminal close journal: {error}"))
}

fn migrate_schema(conn: &Connection) -> rusqlite::Result<()> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let layout_table_exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name = 'pending_layout_closes')",
        [],
        |row| row.get(0),
    )?;
    // The shell can outlive both the layout workspace and its saved host.
    // The captured route is part of its identity and has no foreign key.
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS pending_daemon_closes (
            route TEXT NOT NULL,
            session_id TEXT NOT NULL,
            shell_id BLOB NOT NULL CHECK (length(shell_id) = 16),
            PRIMARY KEY (route, session_id, shell_id)
        );
        CREATE TABLE IF NOT EXISTS pending_layout_closes (
            scope TEXT NOT NULL CHECK (scope IN ('pane', 'session', 'tab', 'workspace', 'archive')),
            workspace_id TEXT NOT NULL,
            target_id TEXT NOT NULL,
            payload TEXT CHECK ((scope = 'archive') = (payload IS NOT NULL)),
            PRIMARY KEY (scope, workspace_id, target_id)
        );",
    )?;
    if !layout_table_exists {
        // Pre-release rows only knew how to restore terminal pane removal.
        // Keep that behavior and discard their layout-only daemon sentinels.
        tx.execute_batch(
            "INSERT INTO pending_layout_closes (scope, workspace_id, target_id)
             SELECT DISTINCT 'session', json_extract(route, '$.workspace_id'), session_id
             FROM pending_daemon_closes;
             DELETE FROM pending_daemon_closes WHERE shell_id = zeroblob(16);",
        )?;
    }
    tx.commit()
}

impl DatabaseStore {
    pub(crate) fn record_layout_close(
        &self,
        intent: &LayoutCloseIntent,
        daemon_closes: &[PendingDaemonClose],
    ) -> Result<(), String> {
        let payload = match intent {
            LayoutCloseIntent::Archive(entry) => {
                Some(serde_json::to_string(entry).map_err(|error| error.to_string())?)
            }
            _ => None,
        };
        let (scope, workspace_id, target_id) = intent.row_key();
        let mut conn = self.conn.lock().unwrap();
        if conn.path().is_none_or(str::is_empty) {
            return Err("Terminal close journal has no on-disk database".into());
        }
        let tx = conn.transaction().map_err(|error| error.to_string())?;
        tx.execute(
            "INSERT INTO pending_layout_closes (scope, workspace_id, target_id, payload)
             VALUES (?1, ?2, ?3, ?4) ON CONFLICT DO NOTHING",
            params![scope, workspace_id, target_id, payload],
        )
        .map_err(|error| error.to_string())?;
        {
            let mut insert = tx
                .prepare_cached(
                    "INSERT INTO pending_daemon_closes (route, session_id, shell_id)
                 VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING",
                )
                .map_err(|error| error.to_string())?;
            for close in daemon_closes {
                let route =
                    serde_json::to_string(&close.route).map_err(|error| error.to_string())?;
                insert
                    .execute(params![
                        route,
                        close.session_id,
                        close.shell_id.as_bytes().as_slice()
                    ])
                    .map_err(|error| error.to_string())?;
            }
        }
        tx.commit().map_err(|error| error.to_string())
    }

    pub(crate) fn pending_layout_closes(&self) -> Result<Vec<LayoutCloseIntent>, String> {
        let conn = self.conn.lock().unwrap();
        let mut select = conn
            .prepare(
                "SELECT scope, workspace_id, target_id, payload FROM pending_layout_closes ORDER BY rowid",
            )
            .map_err(|error| error.to_string())?;
        let rows = select
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(|error| error.to_string())?;
        rows.map(|row| {
            let (scope, workspace_id, target_id, payload) =
                row.map_err(|error| error.to_string())?;
            match scope.as_str() {
                "pane" => Ok(LayoutCloseIntent::Pane(target_id)),
                "session" => Ok(LayoutCloseIntent::Session {
                    workspace_id,
                    session_id: target_id,
                }),
                "tab" => Ok(LayoutCloseIntent::Tab {
                    workspace_id,
                    tab_id: target_id,
                }),
                "workspace" => Ok(LayoutCloseIntent::Workspace(workspace_id)),
                "archive" => {
                    let entry: ArchivedWorkspaceSnapshot = serde_json::from_str(
                        payload
                            .as_deref()
                            .ok_or("Archive close payload is missing")?,
                    )
                    .map_err(|error| error.to_string())?;
                    if entry.workspace_id != workspace_id || entry.archive_id != target_id {
                        return Err("Archive close payload has a different identity".into());
                    }
                    Ok(LayoutCloseIntent::Archive(Box::new(entry)))
                }
                _ => Err(format!("Unknown layout close scope {scope}")),
            }
        })
        .collect()
    }

    pub(crate) fn retire_layout_closes(&self, intents: &[LayoutCloseIntent]) -> Result<(), String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|error| error.to_string())?;
        {
            let mut delete = tx.prepare_cached(
                "DELETE FROM pending_layout_closes WHERE scope = ?1 AND workspace_id = ?2 AND target_id = ?3",
            ).map_err(|error| error.to_string())?;
            for intent in intents {
                delete
                    .execute(intent.row_key())
                    .map_err(|error| error.to_string())?;
            }
        }
        tx.commit().map_err(|error| error.to_string())
    }

    pub(crate) fn has_pending_layout_close(
        &self,
        intent: &LayoutCloseIntent,
    ) -> Result<bool, String> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pending_layout_closes
             WHERE scope = ?1 AND workspace_id = ?2 AND target_id = ?3)",
                intent.row_key(),
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())
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
            let record = |route: &DaemonWorkspaceRoute, shell_id| PendingDaemonClose {
                route: route.clone(),
                session_id: "session".into(),
                shell_id,
            };
            db.record_layout_close(
                &LayoutCloseIntent::Workspace("workspace".into()),
                &[
                    record(&route, first),
                    record(&route, second),
                    record(&route, first),
                    record(&other_route, first),
                ],
            )
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

    #[test]
    fn old_pane_rows_migrate_to_scoped_layout_intents_once() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.sqlite");
        let route = DaemonWorkspaceRoute {
            workspace_id: "workspace".into(),
            host_id: None,
            ssh_target: None,
        };
        let session = LayoutCloseIntent::Session {
            workspace_id: "workspace".into(),
            session_id: "session".into(),
        };
        let shell = Uuid::new_v4();
        {
            let db = DatabaseStore::test_file(&path);
            db.record_layout_close(
                &session,
                &[Uuid::nil(), shell].map(|shell_id| PendingDaemonClose {
                    route: route.clone(),
                    session_id: "session".into(),
                    shell_id,
                }),
            )
            .unwrap();
            db.conn
                .lock()
                .unwrap()
                .execute_batch("DROP TABLE pending_layout_closes")
                .unwrap();
        }
        {
            let db = DatabaseStore::test_file(&path);
            assert_eq!(db.pending_layout_closes().unwrap(), [session.clone()]);
            assert_eq!(
                db.pending_daemon_closes()
                    .unwrap()
                    .iter()
                    .map(|job| job.shell_id)
                    .collect::<Vec<_>>(),
                [shell]
            );
            db.retire_layout_closes(&[session]).unwrap();
        }
        let db = DatabaseStore::test_file(&path);
        assert!(db.pending_layout_closes().unwrap().is_empty());
        assert_eq!(db.pending_daemon_closes().unwrap().len(), 1);
    }
}
