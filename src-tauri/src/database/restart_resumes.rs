//! Turns still running when the app stopped, for the resume after a restart.
//!
//! A row is written when a chat's turn starts and removed when it settles,
//! so the table always lists what is running right now (see
//! `commands::restart_resume`). Each row records the `epoch` (one id per
//! app run) that saw the turn start. On the next launch every row from an
//! earlier epoch is a turn the app stopped mid-flight: a graceful quit
//! stamps `quit_at_ms` on the way out, a crash leaves it empty. The launch
//! takes those rows exactly once.
//!
//! `waiting` lists the approvals and questions the turn was blocked on, and
//! `chain` counts the automatic resumes in a row that led to this turn.
use super::*;

pub(super) fn create_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS agent_chat_restart_resumes (
            thread_id TEXT PRIMARY KEY
                REFERENCES agent_chat_sessions(thread_id) ON DELETE CASCADE,
            turn_id TEXT NOT NULL,
            epoch TEXT NOT NULL,
            waiting TEXT NOT NULL DEFAULT '[]',
            chain INTEGER NOT NULL DEFAULT 0,
            started_at_ms INTEGER NOT NULL,
            quit_at_ms INTEGER
        );
        "#,
    )
    .map_err(|e| format!("Failed to create restart resume schema: {e}"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestartResumeRecord {
    pub thread_id: String,
    pub turn_id: String,
    pub epoch: String,
    /// Open approval and question ids.
    pub waiting: Vec<String>,
    pub chain: i64,
    pub started_at_ms: i64,
    /// Set when the app quit normally; `None` after a crash.
    pub quit_at_ms: Option<i64>,
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<RestartResumeRecord> {
    Ok(RestartResumeRecord {
        thread_id: row.get(0)?,
        turn_id: row.get(1)?,
        epoch: row.get(2)?,
        waiting: serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or_default(),
        chain: row.get(4)?,
        started_at_ms: row.get(5)?,
        quit_at_ms: row.get(6)?,
    })
}

const COLUMNS: &str = "thread_id, turn_id, epoch, waiting, chain, started_at_ms, quit_at_ms";

impl DatabaseStore {
    /// Record that `turn_id` is running on a thread. The same turn reported
    /// again keeps what it is waiting on. A thread without a session row
    /// (a side chat) has nothing to resume, so nothing is written.
    pub fn mark_restart_turn_running(
        &self,
        thread_id: &str,
        turn_id: &str,
        epoch: &str,
        chain: i64,
        now_ms: i64,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO agent_chat_restart_resumes
                 (thread_id, turn_id, epoch, chain, started_at_ms)
             SELECT ?1, ?2, ?3, ?4, ?5
             WHERE EXISTS (SELECT 1 FROM agent_chat_sessions WHERE thread_id = ?1)
             ON CONFLICT(thread_id) DO UPDATE SET
                 waiting = CASE WHEN turn_id = excluded.turn_id AND epoch = excluded.epoch
                           THEN waiting ELSE '[]' END,
                 turn_id = excluded.turn_id,
                 epoch = excluded.epoch,
                 chain = excluded.chain,
                 started_at_ms = excluded.started_at_ms,
                 quit_at_ms = NULL",
            params![thread_id, turn_id, epoch, chain, now_ms],
        )
        .map(drop)
        .map_err(|e| format!("Failed to record running turn: {e}"))
    }

    /// The thread's turn settled or was stopped. Returns whether one was
    /// recorded.
    pub fn clear_restart_turn(&self, thread_id: &str) -> Result<bool, String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM agent_chat_restart_resumes WHERE thread_id = ?1",
            params![thread_id],
        )
        .map(|changed| changed > 0)
        .map_err(|e| format!("Failed to clear running turn: {e}"))
    }

    /// [`clear_restart_turn`](Self::clear_restart_turn), but only while the
    /// recorded turn is still `turn_id`.
    pub fn clear_restart_turn_if(&self, thread_id: &str, turn_id: &str) -> Result<bool, String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM agent_chat_restart_resumes WHERE thread_id = ?1 AND turn_id = ?2",
            params![thread_id, turn_id],
        )
        .map(|changed| changed > 0)
        .map_err(|e| format!("Failed to clear running turn: {e}"))
    }

    /// Add (`open`) or remove an approval or question the running turn waits
    /// on. No-op when the thread has no running turn recorded.
    pub fn set_restart_turn_waiting(
        &self,
        thread_id: &str,
        request_id: &str,
        open: bool,
    ) -> Result<(), String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Failed to start waiting update: {e}"))?;
        let waiting: Option<String> = tx
            .query_row(
                "SELECT waiting FROM agent_chat_restart_resumes WHERE thread_id = ?1",
                params![thread_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to read waiting requests: {e}"))?;
        let Some(waiting) = waiting else {
            return Ok(());
        };
        let mut ids: Vec<String> = serde_json::from_str(&waiting).unwrap_or_default();
        let had = ids.iter().any(|id| id == request_id);
        match (open, had) {
            (true, false) => ids.push(request_id.to_owned()),
            (false, true) => ids.retain(|id| id != request_id),
            _ => return Ok(()),
        }
        let encoded = serde_json::to_string(&ids).map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE agent_chat_restart_resumes SET waiting = ?2 WHERE thread_id = ?1",
            params![thread_id, encoded],
        )
        .map_err(|e| format!("Failed to update waiting requests: {e}"))?;
        tx.commit()
            .map_err(|e| format!("Failed to commit waiting update: {e}"))
    }

    /// Stamp this run's running turns as stopped by a normal quit.
    pub fn stamp_restart_turns_quit(&self, epoch: &str, now_ms: i64) -> Result<usize, String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agent_chat_restart_resumes SET quit_at_ms = ?2 WHERE epoch = ?1",
            params![epoch, now_ms],
        )
        .map_err(|e| format!("Failed to stamp running turns: {e}"))
    }

    /// Remove and return every turn an earlier run left running, oldest
    /// first. Taking them in one step means each is acted on at most once,
    /// even if this launch crashes halfway through resuming them.
    pub fn take_stale_restart_turns(&self, epoch: &str) -> Result<Vec<RestartResumeRecord>, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Failed to start restart resume claim: {e}"))?;
        let rows = {
            let mut statement = tx
                .prepare(&format!(
                    "SELECT {COLUMNS} FROM agent_chat_restart_resumes
                     WHERE epoch != ?1 ORDER BY started_at_ms, thread_id"
                ))
                .map_err(|e| format!("Failed to read running turns: {e}"))?;
            let rows = statement
                .query_map(params![epoch], row_to_record)
                .map_err(|e| format!("Failed to read running turns: {e}"))?
                .filter_map(Result::ok)
                .collect::<Vec<_>>();
            rows
        };
        tx.execute(
            "DELETE FROM agent_chat_restart_resumes WHERE epoch != ?1",
            params![epoch],
        )
        .map_err(|e| format!("Failed to claim running turns: {e}"))?;
        tx.commit()
            .map_err(|e| format!("Failed to commit restart resume claim: {e}"))?;
        Ok(rows)
    }

    pub fn get_restart_turn(&self, thread_id: &str) -> Option<RestartResumeRecord> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            &format!("SELECT {COLUMNS} FROM agent_chat_restart_resumes WHERE thread_id = ?1"),
            params![thread_id],
            row_to_record,
        )
        .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_with(threads: &[&str]) -> DatabaseStore {
        let db = init_test_database();
        for thread in threads {
            db.upsert_agent_chat_session(thread, "w", None, "claude").unwrap();
        }
        db
    }

    #[test]
    fn running_turns_are_recorded_and_cleared() {
        let db = db_with(&["t"]);
        db.mark_restart_turn_running("t", "turn-1", "run-a", 0, 10).unwrap();
        db.set_restart_turn_waiting("t", "req-1", true).unwrap();
        db.set_restart_turn_waiting("t", "req-1", true).unwrap();
        // The same turn reported again keeps its open requests.
        db.mark_restart_turn_running("t", "turn-1", "run-a", 0, 11).unwrap();
        assert_eq!(db.get_restart_turn("t").unwrap().waiting, ["req-1"]);
        db.set_restart_turn_waiting("t", "req-1", false).unwrap();
        assert!(db.get_restart_turn("t").unwrap().waiting.is_empty());
        // A new turn starts with nothing open.
        db.set_restart_turn_waiting("t", "req-2", true).unwrap();
        db.mark_restart_turn_running("t", "turn-2", "run-a", 1, 12).unwrap();
        let row = db.get_restart_turn("t").unwrap();
        assert_eq!((row.turn_id.as_str(), row.chain, row.waiting.len()), ("turn-2", 1, 0));
        assert!(db.clear_restart_turn("t").unwrap());
        assert!(!db.clear_restart_turn("t").unwrap());
        // Waiting on a thread with no running turn records nothing.
        db.set_restart_turn_waiting("t", "req-3", true).unwrap();
        assert!(db.get_restart_turn("t").is_none());
    }

    #[test]
    fn threads_without_a_session_are_not_recorded() {
        let db = db_with(&[]);
        db.mark_restart_turn_running("side-chat", "turn", "run-a", 0, 1).unwrap();
        assert!(db.get_restart_turn("side-chat").is_none());
    }

    #[test]
    fn stale_turns_are_taken_once_and_this_runs_kept() {
        let db = db_with(&["old", "older", "live"]);
        db.mark_restart_turn_running("old", "a", "run-1", 0, 20).unwrap();
        db.mark_restart_turn_running("older", "b", "run-1", 0, 10).unwrap();
        db.mark_restart_turn_running("live", "c", "run-2", 0, 30).unwrap();
        assert_eq!(db.stamp_restart_turns_quit("run-1", 99).unwrap(), 2);
        let taken = db.take_stale_restart_turns("run-2").unwrap();
        let ids: Vec<_> = taken.iter().map(|r| r.thread_id.as_str()).collect();
        assert_eq!(ids, ["older", "old"]);
        assert!(taken.iter().all(|r| r.quit_at_ms == Some(99)));
        assert!(db.take_stale_restart_turns("run-2").unwrap().is_empty());
        assert!(db.get_restart_turn("live").is_some());
    }

    #[test]
    fn deleting_a_session_drops_its_running_turn() {
        let db = db_with(&["t"]);
        db.mark_restart_turn_running("t", "a", "run-1", 0, 1).unwrap();
        db.delete_agent_chat_session("t").unwrap();
        assert!(db.get_restart_turn("t").is_none());
    }
}
