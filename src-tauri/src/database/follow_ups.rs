//! Started suggested follow-up tasks (`commands::follow_ups`).
//!
//! One row per suggestion the user started: which `suggest_tasks` call and
//! index it came from, the child chat and workspace it runs in, the branch
//! and the commit it was made from, and what came of it. The parent's card
//! reads these rows after a reload; Merge all reads the branches from here.
//! No foreign keys, like the delegation journal: a row must outlive the
//! child's session row and workspace.
use super::*;

pub(super) fn create_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_follow_up_tasks (
            child_thread_id TEXT PRIMARY KEY,
            parent_thread_id TEXT NOT NULL,
            tool_call_id TEXT NOT NULL,
            idx INTEGER NOT NULL,
            workspace_id TEXT NOT NULL,
            parent_workspace_id TEXT NOT NULL,
            repo_path TEXT NOT NULL,
            worktree_path TEXT NOT NULL,
            branch TEXT NOT NULL,
            base_sha TEXT NOT NULL,
            provider TEXT NOT NULL,
            title TEXT NOT NULL,
            outcome TEXT NOT NULL DEFAULT 'running',
            additions INTEGER,
            deletions INTEGER,
            commits INTEGER,
            uncommitted INTEGER,
            merged INTEGER NOT NULL DEFAULT 0,
            created_at_ms INTEGER NOT NULL
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_follow_up_tasks_suggestion
            ON agent_follow_up_tasks(parent_thread_id, tool_call_id, idx);",
    )
    .map_err(|e| format!("Failed to create follow-up task table: {e}"))
}

/// One started suggestion. `outcome` is `running`, `completed`, `failed`
/// or `stopped`; the git stats are filled when the task finishes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowUpTaskRow {
    pub child_thread_id: String,
    pub parent_thread_id: String,
    pub tool_call_id: String,
    pub index: u32,
    pub workspace_id: String,
    pub parent_workspace_id: String,
    pub repo_path: String,
    pub worktree_path: String,
    pub branch: String,
    pub base_sha: String,
    pub provider: String,
    pub title: String,
    pub outcome: String,
    pub additions: Option<u32>,
    pub deletions: Option<u32>,
    pub commits: Option<u32>,
    /// Files with uncommitted changes in the child's worktree when it
    /// finished. Such a child is not mergeable.
    pub uncommitted: Option<u32>,
    pub merged: bool,
    pub created_at_ms: i64,
}

const COLUMNS: &str = "child_thread_id, parent_thread_id, tool_call_id, idx, workspace_id, \
    parent_workspace_id, repo_path, worktree_path, branch, base_sha, provider, title, outcome, \
    additions, deletions, commits, uncommitted, merged, created_at_ms";

fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FollowUpTaskRow> {
    Ok(FollowUpTaskRow {
        child_thread_id: row.get(0)?,
        parent_thread_id: row.get(1)?,
        tool_call_id: row.get(2)?,
        index: row.get(3)?,
        workspace_id: row.get(4)?,
        parent_workspace_id: row.get(5)?,
        repo_path: row.get(6)?,
        worktree_path: row.get(7)?,
        branch: row.get(8)?,
        base_sha: row.get(9)?,
        provider: row.get(10)?,
        title: row.get(11)?,
        outcome: row.get(12)?,
        additions: row.get(13)?,
        deletions: row.get(14)?,
        commits: row.get(15)?,
        uncommitted: row.get(16)?,
        merged: row.get::<_, i64>(17)? != 0,
        created_at_ms: row.get(18)?,
    })
}

impl DatabaseStore {
    /// Record a started suggestion. Fails when that suggestion already has
    /// a child, which is what keeps a double click from starting it twice.
    pub fn insert_follow_up_task(&self, task: &FollowUpTaskRow) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            &format!("INSERT INTO agent_follow_up_tasks ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)"),
            params![
                task.child_thread_id,
                task.parent_thread_id,
                task.tool_call_id,
                task.index,
                task.workspace_id,
                task.parent_workspace_id,
                task.repo_path,
                task.worktree_path,
                task.branch,
                task.base_sha,
                task.provider,
                task.title,
                task.outcome,
                task.additions,
                task.deletions,
                task.commits,
                task.uncommitted,
                task.merged as i64,
                task.created_at_ms,
            ],
        )
        .map_err(|e| format!("Failed to record the started task: {e}"))?;
        Ok(())
    }

    /// Fill in the child workspace once it exists.
    pub fn set_follow_up_placement(
        &self,
        child_thread_id: &str,
        workspace_id: &str,
        worktree_path: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agent_follow_up_tasks SET workspace_id = ?2, worktree_path = ?3
             WHERE child_thread_id = ?1",
            params![child_thread_id, workspace_id, worktree_path],
        )
        .map_err(|e| format!("Failed to update the started task: {e}"))?;
        Ok(())
    }

    pub fn delete_follow_up_task(&self, child_thread_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM agent_follow_up_tasks WHERE child_thread_id = ?1",
            params![child_thread_id],
        )
        .map_err(|e| format!("Failed to clear the started task: {e}"))?;
        Ok(())
    }

    pub fn get_follow_up_task(&self, child_thread_id: &str) -> Option<FollowUpTaskRow> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            &format!("SELECT {COLUMNS} FROM agent_follow_up_tasks WHERE child_thread_id = ?1"),
            params![child_thread_id],
            row,
        )
        .optional()
        .ok()
        .flatten()
    }

    pub fn get_follow_up_for_suggestion(
        &self,
        parent_thread_id: &str,
        tool_call_id: &str,
        index: u32,
    ) -> Option<FollowUpTaskRow> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            &format!(
                "SELECT {COLUMNS} FROM agent_follow_up_tasks
                 WHERE parent_thread_id = ?1 AND tool_call_id = ?2 AND idx = ?3"
            ),
            params![parent_thread_id, tool_call_id, index],
            row,
        )
        .optional()
        .ok()
        .flatten()
    }

    /// Every task started from `parent_thread_id`'s suggestions, by
    /// suggestion call then index.
    pub fn list_follow_up_tasks(&self, parent_thread_id: &str) -> Vec<FollowUpTaskRow> {
        let conn = self.conn.lock().unwrap();
        let Ok(mut stmt) = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM agent_follow_up_tasks
             WHERE parent_thread_id = ?1
             ORDER BY created_at_ms ASC, tool_call_id ASC, idx ASC"
        )) else {
            return Vec::new();
        };
        stmt.query_map(params![parent_thread_id], row)
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    pub fn set_follow_up_outcome(
        &self,
        child_thread_id: &str,
        outcome: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agent_follow_up_tasks SET outcome = ?2 WHERE child_thread_id = ?1",
            params![child_thread_id, outcome],
        )
        .map_err(|e| format!("Failed to update the started task: {e}"))?;
        Ok(())
    }

    pub fn set_follow_up_stats(
        &self,
        child_thread_id: &str,
        additions: u32,
        deletions: u32,
        commits: u32,
        uncommitted: Option<u32>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agent_follow_up_tasks
             SET additions = ?2, deletions = ?3, commits = ?4, uncommitted = ?5
             WHERE child_thread_id = ?1",
            params![child_thread_id, additions, deletions, commits, uncommitted],
        )
        .map_err(|e| format!("Failed to update the started task: {e}"))?;
        Ok(())
    }

    /// Every row still marked `running`, oldest first.
    pub fn running_follow_up_tasks(&self) -> Vec<FollowUpTaskRow> {
        let conn = self.conn.lock().unwrap();
        let Ok(mut stmt) = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM agent_follow_up_tasks
             WHERE outcome = 'running'
             ORDER BY created_at_ms ASC, child_thread_id ASC"
        )) else {
            return Vec::new();
        };
        stmt.query_map([], row)
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    pub fn set_follow_up_merged(&self, child_thread_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agent_follow_up_tasks SET merged = 1 WHERE child_thread_id = ?1",
            params![child_thread_id],
        )
        .map_err(|e| format!("Failed to update the started task: {e}"))?;
        Ok(())
    }

    /// Search the parent's persisted transcript for a finished tool call
    /// and return its input. The latest copy wins (Codex re-emits items).
    pub fn find_tool_use_input(
        &self,
        thread_id: &str,
        tool_use_id: &str,
    ) -> Option<serde_json::Value> {
        let conn = self.conn.lock().unwrap();
        let payload: Option<String> = conn
            .query_row(
                "SELECT payload FROM agent_chat_messages
                 WHERE thread_id = ?1
                   AND json_valid(payload)
                   AND json_extract(payload, '$.item.kind') = 'tool_use'
                   AND json_extract(payload, '$.item.tool_use_id') = ?2
                 ORDER BY id DESC LIMIT 1",
                params![thread_id, tool_use_id],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten();
        let event: serde_json::Value = serde_json::from_str(&payload?).ok()?;
        let item = event.get("item")?;
        Some(serde_json::json!({
            "tool_name": item.get("tool_name").cloned().unwrap_or(serde_json::Value::Null),
            "input": item.get("input").cloned().unwrap_or(serde_json::Value::Null),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(child: &str, tool_call_id: &str, index: u32) -> FollowUpTaskRow {
        FollowUpTaskRow {
            child_thread_id: child.into(),
            parent_thread_id: "parent".into(),
            tool_call_id: tool_call_id.into(),
            index,
            workspace_id: String::new(),
            parent_workspace_id: "ws-parent".into(),
            repo_path: "/repo".into(),
            worktree_path: String::new(),
            branch: format!("branch-{child}"),
            base_sha: "a".repeat(40),
            provider: "codex".into(),
            title: format!("Task {child}"),
            outcome: "running".into(),
            additions: None,
            deletions: None,
            commits: None,
            uncommitted: None,
            merged: false,
            created_at_ms: index as i64,
        }
    }

    #[test]
    fn a_suggestion_starts_once_and_rows_round_trip() {
        let db = DatabaseStore::new_in_memory();
        db.insert_follow_up_task(&task("c1", "call", 0)).unwrap();
        assert!(
            db.insert_follow_up_task(&task("c2", "call", 0)).is_err(),
            "same suggestion twice"
        );
        db.insert_follow_up_task(&task("c3", "call", 1)).unwrap();
        db.set_follow_up_placement("c1", "ws-child", "/wt/c1")
            .unwrap();
        db.set_follow_up_outcome("c1", "completed").unwrap();
        db.set_follow_up_stats("c1", 12, 3, 2, Some(0)).unwrap();
        db.set_follow_up_merged("c1").unwrap();

        let row = db
            .get_follow_up_for_suggestion("parent", "call", 0)
            .unwrap();
        assert_eq!(row.child_thread_id, "c1");
        assert_eq!(row.workspace_id, "ws-child");
        assert_eq!(row.worktree_path, "/wt/c1");
        assert_eq!(row.outcome, "completed");
        assert_eq!(
            (row.additions, row.deletions, row.commits),
            (Some(12), Some(3), Some(2))
        );
        assert!(row.merged);
        let all = db.list_follow_up_tasks("parent");
        assert_eq!(all.iter().map(|t| t.index).collect::<Vec<_>>(), vec![0, 1]);
        db.delete_follow_up_task("c3").unwrap();
        assert!(db.get_follow_up_task("c3").is_none());
    }

    #[test]
    fn finds_the_latest_persisted_tool_input() {
        let db = DatabaseStore::new_in_memory();
        db.upsert_agent_chat_session("parent", "ws", None, "claude")
            .unwrap();
        let event = |tasks: &str| {
            format!(
                r#"{{"type":"item_completed","thread_id":"parent","turn_id":"t","item":{{"kind":"tool_use","tool_name":"mcp__codemux__suggest_tasks","tool_use_id":"call-1","input":{{"tasks":[{{"title":"{tasks}"}}]}}}}}}"#
            )
        };
        db.append_agent_chat_message("parent", &event("old"))
            .unwrap();
        db.append_agent_chat_message("parent", &event("new"))
            .unwrap();
        db.append_agent_chat_message("parent", "not json").unwrap();
        let found = db.find_tool_use_input("parent", "call-1").unwrap();
        assert_eq!(found["tool_name"], "mcp__codemux__suggest_tasks");
        assert_eq!(found["input"]["tasks"][0]["title"], "new");
        assert!(db.find_tool_use_input("parent", "other").is_none());
    }
}
