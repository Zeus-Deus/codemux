//! Local journal for layout closes and their independent guarded daemon cleanup.

use super::DatabaseStore;
use crate::agent_control::{ControlError, ErrorCode};
use crate::state::{ArchivedWorkspaceSnapshot, LayoutCloseIntent, SessionId};
use crate::terminal::TerminalCloseError;
use crate::workspace_service::Checkpoint;
use rusqlite::{params, Connection, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::{Duration, Instant};
use uuid::Uuid;

#[cfg(test)]
static DAEMON_OWNERSHIP_WRITE_WATCH: std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedSender<()>>> =
    std::sync::Mutex::new(None);

#[cfg(all(test, unix))]
static CLOSE_JOURNAL_CONTENTION: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>> =
    std::sync::Mutex::new(None);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct DaemonWorkspaceRoute {
    pub(crate) workspace_id: String,
    pub(crate) host_id: Option<i64>,
    pub(crate) ssh_target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
        );
        CREATE TABLE IF NOT EXISTS daemon_shell_ownership (
            route TEXT NOT NULL,
            workspace_id TEXT NOT NULL,
            session_id TEXT NOT NULL,
            shell_id BLOB NOT NULL CHECK (length(shell_id) = 16),
            PRIMARY KEY (route, session_id, shell_id)
        );
        CREATE INDEX IF NOT EXISTS daemon_shell_ownership_session
            ON daemon_shell_ownership (workspace_id, session_id);",
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

fn insert_pending_closes(conn: &Connection, closes: &[PendingDaemonClose]) -> Result<(), String> {
    let mut insert = conn
        .prepare_cached(
            "INSERT INTO pending_daemon_closes (route, session_id, shell_id)
         VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING",
        )
        .map_err(|error| error.to_string())?;
    for close in closes {
        let route = serde_json::to_string(&close.route).map_err(|error| error.to_string())?;
        insert
            .execute(params![
                route,
                close.session_id,
                close.shell_id.as_bytes().as_slice()
            ])
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn delete_owned_shell(conn: &Connection, shell: &PendingDaemonClose) -> Result<usize, String> {
    let route = serde_json::to_string(&shell.route).map_err(|error| error.to_string())?;
    conn.execute(
        "DELETE FROM daemon_shell_ownership WHERE route = ?1 AND session_id = ?2 AND shell_id = ?3",
        params![
            route,
            shell.session_id,
            shell.shell_id.as_bytes().as_slice()
        ],
    )
    .map_err(|error| error.to_string())
}

fn insert_owned_shell(conn: &Connection, shell: &PendingDaemonClose) -> Result<(), String> {
    let route = serde_json::to_string(&shell.route).map_err(|error| error.to_string())?;
    conn.execute(
        "INSERT INTO daemon_shell_ownership (route, workspace_id, session_id, shell_id)
         VALUES (?1, ?2, ?3, ?4) ON CONFLICT DO NOTHING",
        params![
            route,
            shell.route.workspace_id,
            shell.session_id,
            shell.shell_id.as_bytes().as_slice()
        ],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn read_shell_records(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<PendingDaemonClose>, String> {
    let mut select = conn
        .prepare_cached(sql)
        .map_err(|error| error.to_string())?;
    let rows = select
        .query_map(params, |row| {
            Ok((row.get::<_, String>(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(|error| error.to_string())?;
    rows.map(|row| {
        let (route, session_id, shell_id) = row.map_err(|error| error.to_string())?;
        Ok(PendingDaemonClose {
            route: serde_json::from_str(&route).map_err(|error| error.to_string())?,
            session_id,
            shell_id: Uuid::from_bytes(shell_id),
        })
    })
    .collect()
}

fn begin_authorized_close<'a>(
    conn: &'a Connection,
    checkpoint: &Checkpoint,
) -> Result<Transaction<'a>, TerminalCloseError> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let version: i64 = conn.pragma_query_value(None, "data_version", |row| row.get(0))
            .map_err(|error| error.to_string())?;
        checkpoint.check().map_err(TerminalCloseError::Authorization)?;
        match Transaction::new_unchecked(conn, TransactionBehavior::Immediate) {
            Ok(tx) => {
                let current = tx.pragma_query_value(None, "data_version", |row| row.get::<_, i64>(0))
                    .map_err(|error| error.to_string())?;
                // A durable grant may have changed between the checkpoint
                // and writer admission. Reauthorize against the newer data.
                if current == version {
                    return Ok(tx);
                }
            }
            Err(error) if error.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseBusy) => {
                #[cfg(all(test, unix))]
                if let Some(notify) = CLOSE_JOURNAL_CONTENTION.lock().unwrap().take() {
                    let _ = notify.send(());
                }
            }
            Err(error) => return Err(error.to_string().into()),
        }
        if Instant::now() >= deadline {
            return Err("Timed out waiting for terminal close journal".to_owned().into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn write_layout_close(
    tx: Transaction<'_>,
    intent: &LayoutCloseIntent,
    payload: Option<&str>,
    workspace_id: Option<&str>,
    closed_sessions: &[SessionId],
    daemon_closes: &[PendingDaemonClose],
    grant_id: Option<&str>,
) -> Result<Vec<PendingDaemonClose>, TerminalCloseError> {
    let (scope, intent_workspace_id, target_id) = intent.row_key();
    tx.execute(
        "INSERT INTO pending_layout_closes (scope, workspace_id, target_id, payload)
         VALUES (?1, ?2, ?3, ?4) ON CONFLICT DO NOTHING",
        params![scope, intent_workspace_id, target_id, payload],
    )
    .map_err(|error| error.to_string())?;
    let mut closes = daemon_closes.to_vec();
    if let Some(workspace_id) = workspace_id {
        for id in closed_sessions {
            closes.extend(read_shell_records(
                &tx,
                "SELECT route, session_id, shell_id FROM daemon_shell_ownership
                 WHERE workspace_id = ?1 AND session_id = ?2 ORDER BY rowid",
                params![workspace_id, id.0],
            )?);
            tx.execute(
                "DELETE FROM daemon_shell_ownership WHERE workspace_id = ?1 AND session_id = ?2",
                params![workspace_id, id.0],
            ).map_err(|error| error.to_string())?;
        }
    }
    let mut unique = HashSet::new();
    closes.retain(|close| unique.insert(close.clone()));
    insert_pending_closes(&tx, &closes)?;
    if let Some(grant_id) = grant_id {
        // Expiry does not change the database version. Read the grant on
        // this transaction using the clock at the durable commit boundary.
        let live = super::mcp_connector::mcp_grant_is_live(&tx, grant_id, crate::mcp_connector::now_ms())
            .map_err(|error| error.to_string())?;
        if !live {
            let error = ControlError::new(ErrorCode::GrantRevoked, "Grant is expired or revoked");
            return Err(TerminalCloseError::Authorization(error.to_string()));
        }
    }
    tx.commit().map_err(|error| error.to_string())?;
    Ok(closes)
}

impl DatabaseStore {
    #[cfg(test)]
    pub(crate) fn test_watch_daemon_ownership_writes(&self, notify: tokio::sync::mpsc::UnboundedSender<()>) {
        *DAEMON_OWNERSHIP_WRITE_WATCH.lock().unwrap() = Some(notify);
    }

    #[cfg(all(test, unix))]
    pub(crate) fn test_watch_close_journal_contention(&self, notify: tokio::sync::oneshot::Sender<()>) {
        *CLOSE_JOURNAL_CONTENTION.lock().unwrap() = Some(notify);
    }

    pub(crate) fn has_durable_close_journal(&self) -> bool {
        self.conn.lock().unwrap().path().is_some_and(|path| !path.is_empty())
    }

    #[cfg(test)]
    pub(crate) fn record_layout_close(
        &self,
        intent: &LayoutCloseIntent,
        daemon_closes: &[PendingDaemonClose],
    ) -> Result<(), String> {
        self.record_layout_close_with_shell_ownership(intent, None, &[], daemon_closes, &Checkpoint::default())
            .map(drop)
            .map_err(String::from)
    }

    /// A layout removal and all exact shells it owned must survive together.
    pub(crate) fn record_layout_close_with_shell_ownership(
        &self,
        intent: &LayoutCloseIntent,
        workspace_id: Option<&str>,
        closed_sessions: &[SessionId],
        daemon_closes: &[PendingDaemonClose],
        checkpoint: &Checkpoint,
    ) -> Result<Vec<PendingDaemonClose>, TerminalCloseError> {
        let payload = match intent {
            LayoutCloseIntent::Archive(entry) => {
                Some(serde_json::to_string(entry).map_err(|error| error.to_string())?)
            }
            _ => None,
        };
        if checkpoint.requires_check() {
            let path = {
                let conn = self.conn.lock().unwrap();
                conn.path().filter(|path| !path.is_empty()).map(std::path::PathBuf::from)
                    .ok_or_else(|| "Terminal close journal has no on-disk database".to_owned())?
            };
            let conn = super::open_connection(&path)?;
            // A checkpoint may need the main DB and authority locks. Run it
            // without the SQLite writer lock, then only take that lock if
            // it is available immediately and the database is still current.
            conn.busy_timeout(Duration::ZERO).map_err(|error| error.to_string())?;
            let tx = begin_authorized_close(&conn, checkpoint)?;
            return write_layout_close(
                tx, intent, payload.as_deref(), workspace_id, closed_sessions, daemon_closes, checkpoint.connector_grant_id(),
            );
        }
        let mut conn = self.conn.lock().unwrap();
        if conn.path().is_none_or(str::is_empty) {
            return Err(TerminalCloseError::Failed("Terminal close journal has no on-disk database".into()));
        }
        let tx = conn.transaction().map_err(|error| error.to_string())?;
        write_layout_close(tx, intent, payload.as_deref(), workspace_id, closed_sessions, daemon_closes, None)
    }

    pub(crate) fn remember_daemon_shell(&self, shell: &PendingDaemonClose) -> Result<(), String> {
        #[cfg(test)]
        if let Some(notify) = DAEMON_OWNERSHIP_WRITE_WATCH.lock().unwrap().take() {
            let _ = notify.send(());
        }
        let conn = self.conn.lock().unwrap();
        if conn.path().is_none_or(str::is_empty) {
            return Err("Terminal shell ownership has no on-disk database".into());
        }
        insert_owned_shell(&conn, shell)
    }

    /// The caller still owns the live reservation and has proved the other
    /// UUIDs on this daemon cannot survive. Recover a missed earlier insert.
    pub(crate) fn retain_daemon_shell(&self, shell: &PendingDaemonClose) -> Result<(), String> {
        let route = serde_json::to_string(&shell.route).map_err(|error| error.to_string())?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|error| error.to_string())?;
        insert_owned_shell(&tx, shell)?;
        tx.execute(
            "DELETE FROM daemon_shell_ownership WHERE route = ?1 AND session_id = ?2 AND shell_id <> ?3",
            params![route, shell.session_id, shell.shell_id.as_bytes().as_slice()],
        ).map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())
    }

    pub(crate) fn forget_daemon_shell(&self, shell: &PendingDaemonClose) -> Result<(), String> {
        delete_owned_shell(&self.conn.lock().unwrap(), shell).map(drop)
    }

    pub(crate) fn daemon_shell_ownership(&self) -> Result<Vec<PendingDaemonClose>, String> {
        read_shell_records(
            &self.conn.lock().unwrap(),
            "SELECT route, session_id, shell_id FROM daemon_shell_ownership ORDER BY rowid",
            [],
        )
    }

    pub(crate) fn journal_orphan_daemon_shells(
        &self,
        shells: &[PendingDaemonClose],
    ) -> Result<(), String> {
        if shells.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|error| error.to_string())?;
        for shell in shells {
            if delete_owned_shell(&tx, shell)? != 0 {
                insert_pending_closes(&tx, std::slice::from_ref(shell))?;
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
        read_shell_records(
            &self.conn.lock().unwrap(),
            "SELECT route, session_id, shell_id FROM pending_daemon_closes ORDER BY rowid",
            [],
        )
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
    fn memory_database_cannot_record_guarded_shell_ownership() {
        let db = DatabaseStore::new_in_memory();
        let shell = PendingDaemonClose {
            route: DaemonWorkspaceRoute {
                workspace_id: "workspace-1".into(),
                host_id: None,
                ssh_target: None,
            },
            session_id: "session-2".into(),
            shell_id: Uuid::new_v4(),
        };
        assert!(db.remember_daemon_shell(&shell).is_err());
        assert!(db.daemon_shell_ownership().unwrap().is_empty());
    }

    #[test]
    fn dormant_daemon_shell_ownership_moves_atomically_with_its_workspace_close() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state.sqlite");
        let session_id = SessionId("session-7".into());
        let record = |workspace_id: &str, host_id, target: &str| PendingDaemonClose {
            route: DaemonWorkspaceRoute {
                workspace_id: workspace_id.into(),
                host_id: Some(host_id),
                ssh_target: Some(target.into()),
            },
            session_id: session_id.0.clone(),
            shell_id: Uuid::new_v4(),
        };
        let old = record("workspace-1", 1, "original-host");
        let installed = record("workspace-1", 1, "original-host");
        let other_host = record("workspace-1", 2, "other-host");
        let other_workspace = record("workspace-2", 1, "original-host");
        {
            let db = DatabaseStore::test_file(&path);
            db.remember_daemon_shell(&old).unwrap();
            db.remember_daemon_shell(&other_host).unwrap();
            db.remember_daemon_shell(&other_workspace).unwrap();
            db.retain_daemon_shell(&installed).unwrap();
        }

        let intent = LayoutCloseIntent::Workspace("workspace-1".into());
        let transfer = |db: &DatabaseStore| {
            db.record_layout_close_with_shell_ownership(
                &intent,
                Some("workspace-1"),
                std::slice::from_ref(&session_id),
                std::slice::from_ref(&installed),
                &Checkpoint::default(),
            )
            .map_err(String::from)
        };
        {
            let db = DatabaseStore::test_file(&path);
            db.conn
                .lock()
                .unwrap()
                .execute_batch(
                    "CREATE TRIGGER block_pending_close BEFORE INSERT ON pending_daemon_closes
                 BEGIN SELECT RAISE(ABORT, 'fixture cannot record close'); END;",
                )
                .unwrap();
            assert!(transfer(&db).is_err());
            assert!(db.pending_layout_closes().unwrap().is_empty());
            assert_eq!(
                db.daemon_shell_ownership().unwrap(),
                [
                    other_host.clone(),
                    other_workspace.clone(),
                    installed.clone()
                ]
            );
            db.conn
                .lock()
                .unwrap()
                .execute_batch("DROP TRIGGER block_pending_close")
                .unwrap();
            assert_eq!(
                transfer(&db).unwrap(),
                [installed.clone(), other_host.clone()]
            );
        }

        let db = DatabaseStore::test_file(&path);
        assert_eq!(db.pending_layout_closes().unwrap(), [intent]);
        assert_eq!(db.pending_daemon_closes().unwrap(), [installed, other_host]);
        assert_eq!(
            db.daemon_shell_ownership().unwrap(),
            [other_workspace.clone()]
        );
        db.journal_orphan_daemon_shells(std::slice::from_ref(&other_workspace))
            .unwrap();
        assert!(db.daemon_shell_ownership().unwrap().is_empty());
        assert_eq!(
            db.pending_daemon_closes().unwrap().last(),
            Some(&other_workspace)
        );
    }

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
