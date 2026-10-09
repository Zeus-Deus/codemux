//! Restart journal for cross-provider delegation.
//!
//! Delegated tasks live in memory (`commands::delegation`); this table only
//! remembers which tasks were still open, so the next launch can mark their
//! cards Stopped instead of leaving them spinning forever. Deliberately no
//! foreign keys: `collapse_duplicate_agent_chat_sessions` deletes session
//! rows through the cascade, and a journal row must outlive that.
use super::*;

pub(super) fn create_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_delegated_open_tasks (
            child_thread_id TEXT PRIMARY KEY,
            parent_thread_id TEXT NOT NULL,
            provider TEXT NOT NULL,
            title TEXT NOT NULL,
            created_at_ms INTEGER NOT NULL
        );",
    )
    .map_err(|e| format!("Failed to create delegated task journal: {e}"))
}

/// One delegated task that had not finished when it was journaled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegatedOpenTask {
    pub child_thread_id: String,
    pub parent_thread_id: String,
    /// Lowercase provider id (`"codex"`, `"claude"`, ...).
    pub provider: String,
    pub title: String,
    pub created_at_ms: i64,
}

impl DatabaseStore {
    pub fn insert_delegated_open_task(&self, task: &DelegatedOpenTask) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO agent_delegated_open_tasks
                 (child_thread_id, parent_thread_id, provider, title, created_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                task.child_thread_id,
                task.parent_thread_id,
                task.provider,
                task.title,
                task.created_at_ms
            ],
        )
        .map_err(|e| format!("Failed to journal delegated task: {e}"))?;
        Ok(())
    }

    pub fn delete_delegated_open_task(&self, child_thread_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM agent_delegated_open_tasks WHERE child_thread_id = ?1",
            params![child_thread_id],
        )
        .map_err(|e| format!("Failed to clear delegated task: {e}"))?;
        Ok(())
    }

    /// Take every journaled task, oldest first, and empty the journal in the
    /// same transaction so a crash mid-drain cannot report a task twice.
    pub fn drain_delegated_open_tasks(&self) -> Result<Vec<DelegatedOpenTask>, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Failed to open delegated journal drain: {e}"))?;
        let tasks = {
            let mut stmt = tx
                .prepare(
                    "SELECT child_thread_id, parent_thread_id, provider, title, created_at_ms
                     FROM agent_delegated_open_tasks
                     ORDER BY created_at_ms ASC, child_thread_id ASC",
                )
                .map_err(|e| format!("Failed to read delegated journal: {e}"))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(DelegatedOpenTask {
                        child_thread_id: row.get(0)?,
                        parent_thread_id: row.get(1)?,
                        provider: row.get(2)?,
                        title: row.get(3)?,
                        created_at_ms: row.get(4)?,
                    })
                })
                .map_err(|e| format!("Failed to read delegated journal: {e}"))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| format!("Failed to read delegated journal: {e}"))?
        };
        tx.execute("DELETE FROM agent_delegated_open_tasks", [])
            .map_err(|e| format!("Failed to empty delegated journal: {e}"))?;
        tx.commit()
            .map_err(|e| format!("Failed to commit delegated journal drain: {e}"))?;
        Ok(tasks)
    }

    /// Whether the thread has an async question the user has not answered or
    /// dismissed yet. A delegated child waiting on one is not finished.
    pub fn has_open_async_question(&self, thread_id: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM agent_chat_questions
                 WHERE thread_id = ?1
                   AND json_extract(resolution_json, '$.status') NOT IN ('answered', 'dismissed')
             )",
            params![thread_id],
            |row| row.get(0),
        )
        .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(child: &str, parent: &str, created_at_ms: i64) -> DelegatedOpenTask {
        DelegatedOpenTask {
            child_thread_id: child.into(),
            parent_thread_id: parent.into(),
            provider: "codex".into(),
            title: format!("Task {child}"),
            created_at_ms,
        }
    }

    #[test]
    fn delegated_journal_drains_once_in_creation_order() {
        let db = DatabaseStore::new_in_memory();
        db.insert_delegated_open_task(&task("child-b", "parent", 20))
            .unwrap();
        db.insert_delegated_open_task(&task("child-a", "parent", 10))
            .unwrap();
        db.insert_delegated_open_task(&task("child-c", "other", 30))
            .unwrap();
        db.delete_delegated_open_task("child-c").unwrap();

        let drained = db.drain_delegated_open_tasks().unwrap();
        assert_eq!(
            drained,
            vec![task("child-a", "parent", 10), task("child-b", "parent", 20)]
        );
        assert!(db.drain_delegated_open_tasks().unwrap().is_empty());
    }

    #[test]
    fn delegated_journal_survives_without_a_session_row() {
        // No FK: the journal must not depend on the session row existing.
        let db = DatabaseStore::new_in_memory();
        db.insert_delegated_open_task(&task("orphan", "missing-parent", 1))
            .unwrap();
        assert_eq!(db.drain_delegated_open_tasks().unwrap().len(), 1);
    }

    #[test]
    fn delegated_open_question_check_ignores_settled_questions() {
        let db = DatabaseStore::new_in_memory();
        db.upsert_agent_chat_session("child", "ws", None, "codex")
            .unwrap();
        assert!(!db.has_open_async_question("child"));
        let question = crate::agent_provider::UserQuestionSet {
            id: "q1".into(),
            target: "target".into(),
            source_item_id: "item-1".into(),
            source_turn_id: "turn-1".into(),
            text: "Which file?".into(),
            questions: Vec::new(),
            subagent_id: None,
        };
        db.record_async_question("child", &question).unwrap();
        assert!(db.has_open_async_question("child"));
        assert!(!db.has_open_async_question("other"));
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "UPDATE agent_chat_questions SET resolution_json = '{\"status\":\"answered\"}'",
                [],
            )
            .unwrap();
        }
        assert!(!db.has_open_async_question("child"));
    }
}
