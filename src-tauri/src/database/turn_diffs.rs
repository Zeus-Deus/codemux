//! Snapshot pairs bracketing each agent turn, for per-turn git diffs.
//!
//! A row is written when a dispatch is accepted (its `before` snapshot),
//! bound to the provider turn id by the dispatch's client nonce, and closed
//! with an `after` snapshot when the turn settles. Unlike
//! `agent_chat_turn_checkpoints` there is no timeline to keep contiguous:
//! a missing row only means that turn has no diff.
use super::*;

pub(super) fn create_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS agent_chat_turn_diffs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            thread_id TEXT NOT NULL
                REFERENCES agent_chat_sessions(thread_id) ON DELETE CASCADE,
            client_nonce TEXT,
            turn_id TEXT,
            repo_path TEXT NOT NULL,
            before_ref TEXT NOT NULL,
            before_commit TEXT NOT NULL,
            after_ref TEXT NOT NULL,
            after_commit TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            completed_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_agent_chat_turn_diffs_thread
            ON agent_chat_turn_diffs(thread_id, id);
        CREATE INDEX IF NOT EXISTS idx_agent_chat_turn_diffs_turn
            ON agent_chat_turn_diffs(thread_id, turn_id);
        "#,
    )
    .map_err(|e| format!("Failed to create turn diff schema: {e}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentChatTurnDiffRecord {
    pub id: i64,
    pub thread_id: String,
    pub client_nonce: Option<String>,
    pub turn_id: Option<String>,
    pub repo_path: String,
    pub before_ref: String,
    pub before_commit: String,
    /// Reserved when the row is written; the ref exists once `after_commit`
    /// is set.
    pub after_ref: String,
    /// `None` while the turn is still running.
    pub after_commit: Option<String>,
}

/// What [`DatabaseStore::insert_agent_chat_turn_diff`] needs; ids and
/// timestamps are assigned by SQLite.
#[derive(Debug, Clone)]
pub struct NewAgentChatTurnDiff {
    pub thread_id: String,
    pub client_nonce: Option<String>,
    pub turn_id: Option<String>,
    pub repo_path: String,
    pub before_ref: String,
    pub before_commit: String,
    pub after_ref: String,
}

const COLUMNS: &str = "id, thread_id, client_nonce, turn_id, repo_path, before_ref,
     before_commit, after_ref, after_commit";

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentChatTurnDiffRecord> {
    Ok(AgentChatTurnDiffRecord {
        id: row.get(0)?,
        thread_id: row.get(1)?,
        client_nonce: row.get(2)?,
        turn_id: row.get(3)?,
        repo_path: row.get(4)?,
        before_ref: row.get(5)?,
        before_commit: row.get(6)?,
        after_ref: row.get(7)?,
        after_commit: row.get(8)?,
    })
}

fn turn_diff_by_id(conn: &Connection, id: i64) -> Result<AgentChatTurnDiffRecord, String> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM agent_chat_turn_diffs WHERE id = ?1"),
        params![id],
        row_to_record,
    )
    .map_err(|e| format!("Failed to read turn diff: {e}"))
}

/// Refs a deleted row leaves behind: `(repo_path, ref_name)`.
pub type TurnDiffRefs = Vec<(String, String)>;

/// Hand a merged-away thread's turn diffs to the thread its transcript moved
/// to, before the cascade would drop them. `filter` selects the source rows
/// and must bind the survivor's thread id as `?2`. Rows keep their own ref
/// names, so their snapshots stay where they are.
pub(super) fn move_to_thread(
    conn: &Connection,
    filter: &str,
    params: impl rusqlite::Params,
) -> Result<(), String> {
    conn.execute(
        &format!("UPDATE agent_chat_turn_diffs SET thread_id = ?2 WHERE {filter}"),
        params,
    )
    .map(|_| ())
    .map_err(|e| format!("Failed to migrate turn diffs: {e}"))
}

impl DatabaseStore {
    /// Record an accepted dispatch's `before` snapshot, then drop rows past
    /// the newest `keep` for the thread. Returns the new row and the refs the
    /// pruned rows held.
    pub fn insert_agent_chat_turn_diff(
        &self,
        row: &NewAgentChatTurnDiff,
        keep: usize,
    ) -> Result<(AgentChatTurnDiffRecord, TurnDiffRefs), String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Failed to start turn diff insert: {e}"))?;
        tx.execute(
            "INSERT INTO agent_chat_turn_diffs
                 (thread_id, client_nonce, turn_id, repo_path, before_ref,
                  before_commit, after_ref)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                row.thread_id,
                row.client_nonce,
                row.turn_id,
                row.repo_path,
                row.before_ref,
                row.before_commit,
                row.after_ref,
            ],
        )
        .map_err(|e| format!("Failed to persist turn diff: {e}"))?;
        let record = turn_diff_by_id(&tx, tx.last_insert_rowid())?;
        let pruned = {
            let mut statement = tx
                .prepare(
                    "SELECT repo_path, before_ref, after_ref FROM agent_chat_turn_diffs
                     WHERE thread_id = ?1
                     ORDER BY id DESC LIMIT -1 OFFSET ?2",
                )
                .map_err(|e| format!("Failed to prepare turn diff prune: {e}"))?;
            let rows = statement
                .query_map(params![row.thread_id, keep as i64], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
                })
                .map_err(|e| format!("Failed to read turn diff prune: {e}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to collect turn diff prune: {e}"))?;
            rows.into_iter()
                .flat_map(|(repo, before, after)| [(repo.clone(), before), (repo, after)])
                .collect::<Vec<_>>()
        };
        tx.execute(
            "DELETE FROM agent_chat_turn_diffs
             WHERE thread_id = ?1 AND id NOT IN (
                 SELECT id FROM agent_chat_turn_diffs
                 WHERE thread_id = ?1 ORDER BY id DESC LIMIT ?2
             )",
            params![row.thread_id, keep as i64],
        )
        .map_err(|e| format!("Failed to prune turn diffs: {e}"))?;
        tx.commit()
            .map_err(|e| format!("Failed to commit turn diff insert: {e}"))?;
        Ok((record, pruned))
    }

    /// Rows of a thread whose turn has not settled yet, oldest first.
    pub fn open_agent_chat_turn_diffs(&self, thread_id: &str) -> Vec<AgentChatTurnDiffRecord> {
        let conn = self.conn.lock().unwrap();
        let Ok(mut statement) = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM agent_chat_turn_diffs
             WHERE thread_id = ?1 AND after_commit IS NULL ORDER BY id"
        )) else {
            return Vec::new();
        };
        statement
            .query_map(params![thread_id], row_to_record)
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    /// Settle a turn. A row that is already closed keeps its first snapshot.
    pub fn close_agent_chat_turn_diff(&self, id: i64, after_commit: &str) -> Result<bool, String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agent_chat_turn_diffs
             SET after_commit = ?2, completed_at = datetime('now')
             WHERE id = ?1 AND after_commit IS NULL",
            params![id, after_commit],
        )
        .map(|changed| changed > 0)
        .map_err(|e| format!("Failed to close turn diff: {e}"))
    }

    /// Give the row created for `client_nonce` its provider turn id. `None`
    /// when no unbound row carries that nonce.
    pub fn bind_agent_chat_turn_diff(
        &self,
        thread_id: &str,
        client_nonce: &str,
        turn_id: &str,
    ) -> Result<Option<AgentChatTurnDiffRecord>, String> {
        let conn = self.conn.lock().unwrap();
        let id: Option<i64> = conn
            .query_row(
                "SELECT id FROM agent_chat_turn_diffs
                 WHERE thread_id = ?1 AND client_nonce = ?2 AND turn_id IS NULL
                 ORDER BY id DESC LIMIT 1",
                params![thread_id, client_nonce],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to find turn diff to bind: {e}"))?;
        let Some(id) = id else {
            return Ok(None);
        };
        conn.execute(
            "UPDATE agent_chat_turn_diffs SET turn_id = ?2 WHERE id = ?1",
            params![id, turn_id],
        )
        .map_err(|e| format!("Failed to bind turn diff: {e}"))?;
        turn_diff_by_id(&conn, id).map(Some)
    }

    pub fn get_agent_chat_turn_diff(
        &self,
        thread_id: &str,
        turn_id: &str,
    ) -> Option<AgentChatTurnDiffRecord> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            &format!(
                "SELECT {COLUMNS} FROM agent_chat_turn_diffs
                 WHERE thread_id = ?1 AND turn_id = ?2 ORDER BY id DESC LIMIT 1"
            ),
            params![thread_id, turn_id],
            row_to_record,
        )
        .ok()
    }

    /// Every ref a thread's rows hold, for cleanup when the thread is
    /// deleted. Rows merged in from another thread keep refs named after it.
    pub fn agent_chat_turn_diff_refs(&self, thread_id: &str) -> TurnDiffRefs {
        let conn = self.conn.lock().unwrap();
        let Ok(mut statement) = conn.prepare(
            "SELECT repo_path, before_ref, after_ref FROM agent_chat_turn_diffs
             WHERE thread_id = ?1",
        ) else {
            return Vec::new();
        };
        statement
            .query_map(params![thread_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
            })
            .map(|rows| {
                rows.filter_map(Result::ok)
                    .flat_map(|(repo, before, after)| [(repo.clone(), before), (repo, after)])
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_row(thread_id: &str, nonce: &str, token: &str) -> NewAgentChatTurnDiff {
        NewAgentChatTurnDiff {
            thread_id: thread_id.to_string(),
            client_nonce: Some(nonce.to_string()),
            turn_id: None,
            repo_path: "/repo".to_string(),
            before_ref: format!("refs/codemux/turn-diffs/{thread_id}/{token}/before"),
            before_commit: format!("before-{token}"),
            after_ref: format!("refs/codemux/turn-diffs/{thread_id}/{token}/after"),
        }
    }

    #[test]
    fn turn_diff_rows_bind_close_and_prune() {
        let db = DatabaseStore::new_in_memory();
        db.upsert_agent_chat_session("t", "ws", Some("/repo"), "claude")
            .unwrap();

        let (first, pruned) = db
            .insert_agent_chat_turn_diff(&new_row("t", "n1", "1"), 2)
            .unwrap();
        assert!(pruned.is_empty());
        assert_eq!(db.open_agent_chat_turn_diffs("t"), vec![first.clone()]);
        assert!(db.get_agent_chat_turn_diff("t", "turn-1").is_none());

        let bound = db.bind_agent_chat_turn_diff("t", "n1", "turn-1").unwrap().unwrap();
        assert_eq!(bound.turn_id.as_deref(), Some("turn-1"));
        // Already bound: a repeat finds nothing.
        assert!(db.bind_agent_chat_turn_diff("t", "n1", "turn-x").unwrap().is_none());
        assert!(db.bind_agent_chat_turn_diff("t", "missing", "turn-x").unwrap().is_none());

        assert!(db.close_agent_chat_turn_diff(first.id, "after-1").unwrap());
        assert!(!db.close_agent_chat_turn_diff(first.id, "later").unwrap());
        let read = db.get_agent_chat_turn_diff("t", "turn-1").unwrap();
        assert_eq!(read.after_commit.as_deref(), Some("after-1"));
        assert!(db.open_agent_chat_turn_diffs("t").is_empty());

        db.insert_agent_chat_turn_diff(&new_row("t", "n2", "2"), 2).unwrap();
        let (_, pruned) = db
            .insert_agent_chat_turn_diff(&new_row("t", "n3", "3"), 2)
            .unwrap();
        assert_eq!(
            pruned,
            vec![
                ("/repo".to_string(), first.before_ref.clone()),
                ("/repo".to_string(), first.after_ref.clone()),
            ]
        );
        assert!(db.get_agent_chat_turn_diff("t", "turn-1").is_none());
        assert_eq!(db.agent_chat_turn_diff_refs("t").len(), 4);

        db.delete_agent_chat_session("t").unwrap();
        assert!(db.open_agent_chat_turn_diffs("t").is_empty());
        assert!(db.agent_chat_turn_diff_refs("t").is_empty());
    }

    #[test]
    fn resumed_sessions_keep_their_turn_diffs() {
        let db = DatabaseStore::new_in_memory();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute_batch(
                "INSERT INTO agent_chat_sessions
                     (thread_id, sdk_session_id, workspace_id, provider, created_at, last_active_at)
                 VALUES ('old', 'sdk-1', 'ws', 'codex', '2025-01-01', '2025-01-01'),
                        ('new', 'sdk-1', 'ws', 'codex', '2025-03-01', '2025-03-01')",
            )
            .unwrap();
        }
        let (row, _) = db.insert_agent_chat_turn_diff(&new_row("old", "n1", "1"), 10).unwrap();
        db.bind_agent_chat_turn_diff("old", "n1", "turn-1").unwrap();
        db.collapse_duplicate_agent_chat_sessions("sdk-1").unwrap();
        let moved = db.get_agent_chat_turn_diff("new", "turn-1").unwrap();
        assert_eq!(moved.id, row.id);
        assert_eq!(moved.before_ref, row.before_ref);

        db.upsert_agent_chat_session("other", "ws", None, "codex").unwrap();
        db.migrate_agent_chat_session("new", "other").unwrap();
        assert_eq!(db.get_agent_chat_turn_diff("other", "turn-1").unwrap().id, row.id);
    }
}
