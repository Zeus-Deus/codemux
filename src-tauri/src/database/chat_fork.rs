//! Chat forks (`commands::chat_fork`).
//!
//! A fork is an ordinary chat thread: its own `agent_chat_sessions` row, its
//! own copy of the source transcript up to the fork point, and the provider's
//! forked session as its resume cursor. This table only records lineage, so
//! the fork can say where it came from. The row cascades with the fork; it
//! deliberately has no foreign key to the source, which may be deleted
//! while the fork lives on.
use super::*;

pub(super) fn create_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_chat_forks (
            thread_id TEXT PRIMARY KEY,
            source_thread_id TEXT NOT NULL,
            source_title TEXT,
            -- The source row the fork was cut before; NULL for the latest.
            source_cut_message_id INTEGER,
            -- The fork's last copied row: later rows are its own.
            boundary_message_id INTEGER,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (thread_id)
                REFERENCES agent_chat_sessions(thread_id)
                ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_agent_chat_forks_source
            ON agent_chat_forks(source_thread_id);",
    )
    .map_err(|e| format!("Failed to create chat fork table: {e}"))
}

/// The new thread a fork creates: its session row (copied from the source,
/// with the forked provider session as its resume cursor) and lineage.
#[derive(Debug, Clone)]
pub struct NewChatFork<'a> {
    pub thread_id: &'a str,
    pub source: &'a AgentChatSessionRecord,
    pub sdk_session_id: &'a str,
    pub workspace_id: &'a str,
    pub cwd: &'a str,
    pub title: &'a str,
    /// Copy only rows before this source row id; `None` copies them all.
    pub cut_before: Option<i64>,
}

/// Where a fork came from, for the "Forked from" marker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChatForkOrigin {
    pub source_thread_id: String,
    /// The source's title now, or the one it had when forked if it was
    /// deleted since.
    pub source_title: Option<String>,
    pub source_exists: bool,
    pub boundary_message_id: Option<i64>,
}

/// Rewrite one copied transcript row for the fork: the envelope's
/// `thread_id` names the fork, and a user-turn journal row points at the
/// fork's copy of its user message.
pub(crate) fn rewrite_forked_payload(
    payload: &str,
    fork_thread_id: &str,
    copied_ids: &HashMap<i64, i64>,
) -> String {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return payload.to_string();
    };
    let Some(object) = value.as_object_mut() else {
        return payload.to_string();
    };
    if object.get("thread_id").is_some_and(serde_json::Value::is_string) {
        object.insert("thread_id".into(), fork_thread_id.into());
    }
    if object.get("type").and_then(serde_json::Value::as_str) == Some("native_user_control") {
        let source_row = object.get("user_message_id").and_then(serde_json::Value::as_i64);
        if let Some(copied) = source_row.and_then(|row| copied_ids.get(&row)) {
            object.insert("user_message_id".into(), (*copied).into());
        }
    }
    value.to_string()
}

impl DatabaseStore {
    /// Create the fork thread in one transaction: its session row, its copy
    /// of the source transcript, and its lineage row. Fails if the thread id
    /// is already taken, so a retried fork never merges into another chat.
    pub fn create_agent_chat_fork(&self, fork: &NewChatFork<'_>) -> Result<(), String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let source = fork.source;
        tx.execute(
            "INSERT INTO agent_chat_sessions
                 (thread_id, sdk_session_id, workspace_id, cwd, provider, title,
                  model, effort, context_window, permission_mode, fast_mode, service_tier,
                  created_at, last_active_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                     datetime('now'), datetime('now'))",
            params![
                fork.thread_id,
                fork.sdk_session_id,
                fork.workspace_id,
                fork.cwd,
                source.provider,
                fork.title,
                source.model,
                source.effort,
                source.context_window,
                source.permission_mode,
                source.fast_mode as i64,
                source.service_tier,
            ],
        )
        .map_err(|e| format!("Failed to create the fork's session: {e}"))?;
        let rows: Vec<(i64, String, String)> = {
            let mut stmt = tx
                .prepare(
                    "SELECT id, payload, created_at FROM agent_chat_messages
                     WHERE thread_id = ?1 AND (?2 IS NULL OR id < ?2)
                     ORDER BY id ASC",
                )
                .map_err(|e| e.to_string())?;
            let mapped = stmt
                .query_map(params![source.thread_id, fork.cut_before], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .map_err(|e| e.to_string())?;
            mapped.collect::<Result<_, _>>().map_err(|e| e.to_string())?
        };
        let mut copied_ids = HashMap::new();
        let mut boundary = None;
        for (id, payload, created_at) in rows {
            let payload = rewrite_forked_payload(&payload, fork.thread_id, &copied_ids);
            tx.execute(
                "INSERT INTO agent_chat_messages (thread_id, payload, created_at)
                 VALUES (?1, ?2, ?3)",
                params![fork.thread_id, payload, created_at],
            )
            .map_err(|e| format!("Failed to copy the transcript: {e}"))?;
            let copied = tx.last_insert_rowid();
            copied_ids.insert(id, copied);
            boundary = Some(copied);
        }
        tx.execute(
            "INSERT INTO agent_chat_forks
                 (thread_id, source_thread_id, source_title, source_cut_message_id, boundary_message_id)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                fork.thread_id,
                source.thread_id,
                source.title,
                fork.cut_before,
                boundary,
            ],
        )
        .map_err(|e| format!("Failed to record the fork: {e}"))?;
        tx.commit().map_err(|e| e.to_string())
    }

    /// Lineage of `thread_id`, or `None` when it is not a fork.
    pub fn agent_chat_fork_origin(&self, thread_id: &str) -> Option<ChatForkOrigin> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT f.source_thread_id, COALESCE(s.title, f.source_title),
                        s.thread_id IS NOT NULL, f.boundary_message_id
                 FROM agent_chat_forks f
                 LEFT JOIN agent_chat_sessions s ON s.thread_id = f.source_thread_id
                 WHERE f.thread_id = ?1",
                params![thread_id],
                |row| {
                    Ok(ChatForkOrigin {
                        source_thread_id: row.get(0)?,
                        source_title: row.get(1)?,
                        source_exists: row.get(2)?,
                        boundary_message_id: row.get(3)?,
                    })
                },
            )
            .optional()
            .ok()
            .flatten()
    }

    /// The source rows a fork cut needs: the `user_message` rows before
    /// `before` (their text, in order) and the row at `before` itself, which
    /// must be a user message that starts its own turn.
    pub fn agent_chat_fork_prompts(
        &self,
        thread_id: &str,
        before: i64,
    ) -> Result<(Vec<String>, String), String> {
        let conn = self.conn.lock().unwrap();
        let cut: Option<String> = conn
            .query_row(
                "SELECT payload FROM agent_chat_messages WHERE thread_id = ?1 AND id = ?2",
                params![thread_id, before],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let cut = cut
            .as_deref()
            .and_then(|payload| serde_json::from_str::<serde_json::Value>(payload).ok())
            .filter(|value| value.get("type").and_then(serde_json::Value::as_str) == Some("user_message"))
            .filter(|value| value.get("steered_turn_id").is_none())
            .and_then(|value| value.get("text").and_then(serde_json::Value::as_str).map(str::to_string))
            .ok_or("That message doesn't start a turn, so the chat can't be forked there.")?;
        let mut stmt = conn
            .prepare(
                "SELECT json_extract(payload, '$.text') FROM agent_chat_messages
                 WHERE thread_id = ?1 AND id < ?2
                   AND json_valid(payload) AND json_extract(payload, '$.type') = 'user_message'
                 ORDER BY id ASC",
            )
            .map_err(|e| e.to_string())?;
        let earlier = stmt
            .query_map(params![thread_id, before], |row| {
                row.get::<_, Option<String>>(0).map(Option::unwrap_or_default)
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok((earlier, cut))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with_source() -> DatabaseStore {
        let db = DatabaseStore::new_in_memory();
        db.upsert_agent_chat_session("src", "ws", Some("/repo"), "claude").unwrap();
        db.set_agent_chat_title("src", "Fix the parser").unwrap();
        db
    }

    fn append(db: &DatabaseStore, payload: serde_json::Value) -> i64 {
        db.append_agent_chat_message("src", &payload.to_string()).unwrap().unwrap()
    }

    #[test]
    fn rewrite_points_copied_rows_at_the_fork() {
        let ids = HashMap::from([(7, 70)]);
        let row = rewrite_forked_payload(
            r#"{"type":"native_user_control","thread_id":"src","user_message_id":7}"#,
            "fork",
            &ids,
        );
        let row: serde_json::Value = serde_json::from_str(&row).unwrap();
        assert_eq!(row["thread_id"], "fork");
        assert_eq!(row["user_message_id"], 70);
        assert_eq!(rewrite_forked_payload("not json", "fork", &ids), "not json");
        let untouched = rewrite_forked_payload(r#"{"type":"turn_completed"}"#, "fork", &ids);
        assert_eq!(untouched, r#"{"type":"turn_completed"}"#);
    }

    #[test]
    fn fork_copies_the_transcript_up_to_the_cut_and_records_lineage() {
        let db = store_with_source();
        append(&db, serde_json::json!({"type":"user_message","thread_id":"src","text":"one"}));
        append(&db, serde_json::json!({"type":"turn_completed","thread_id":"src"}));
        let second = append(&db, serde_json::json!({"type":"user_message","thread_id":"src","text":"two"}));
        append(&db, serde_json::json!({"type":"turn_completed","thread_id":"src"}));
        let source = db.get_agent_chat_session("src").unwrap();
        db.create_agent_chat_fork(&NewChatFork {
            thread_id: "fork",
            source: &source,
            sdk_session_id: "native-fork",
            workspace_id: "ws2",
            cwd: "/worktree",
            title: "Fix the parser (fork)",
            cut_before: Some(second),
        })
        .unwrap();

        let copied = db.list_agent_chat_messages("fork");
        assert_eq!(copied.len(), 2, "rows from the cut on stay in the source");
        assert!(copied.iter().all(|row| row.contains(r#""thread_id":"fork""#)));
        let record = db.get_agent_chat_session("fork").unwrap();
        assert_eq!(record.sdk_session_id.as_deref(), Some("native-fork"));
        assert_eq!(record.workspace_id, "ws2");
        assert_eq!(record.cwd.as_deref(), Some("/worktree"));
        assert_eq!(record.provider, "claude");
        let origin = db.agent_chat_fork_origin("fork").unwrap();
        assert_eq!(origin.source_thread_id, "src");
        assert_eq!(origin.source_title.as_deref(), Some("Fix the parser"));
        assert!(origin.source_exists);
        assert_eq!(origin.boundary_message_id, db.max_agent_chat_message_id("fork"));
        assert_eq!(db.list_agent_chat_messages("src").len(), 4, "the source is untouched");
        assert!(db.agent_chat_fork_origin("src").is_none());
    }

    #[test]
    fn a_fork_never_reuses_an_existing_thread() {
        let db = store_with_source();
        let source = db.get_agent_chat_session("src").unwrap();
        let fork = NewChatFork {
            thread_id: "src",
            source: &source,
            sdk_session_id: "x",
            workspace_id: "ws",
            cwd: "/repo",
            title: "t",
            cut_before: None,
        };
        assert!(db.create_agent_chat_fork(&fork).is_err());
        assert!(db.agent_chat_fork_origin("src").is_none(), "nothing half-written");
    }

    #[test]
    fn fork_prompts_need_a_turn_starting_user_message() {
        let db = store_with_source();
        append(&db, serde_json::json!({"type":"user_message","text":"one"}));
        let reply = append(&db, serde_json::json!({"type":"turn_completed"}));
        let steered = append(&db, serde_json::json!({"type":"user_message","text":"also","steered_turn_id":"t1"}));
        let next = append(&db, serde_json::json!({"type":"user_message","text":"two"}));
        assert_eq!(
            db.agent_chat_fork_prompts("src", next).unwrap(),
            (vec!["one".to_string(), "also".to_string()], "two".to_string()),
        );
        assert!(db.agent_chat_fork_prompts("src", reply).is_err());
        assert!(db.agent_chat_fork_prompts("src", steered).is_err());
        assert!(db.agent_chat_fork_prompts("other", next).is_err());
    }

    #[test]
    fn origin_outlives_a_deleted_source() {
        let db = store_with_source();
        let source = db.get_agent_chat_session("src").unwrap();
        db.create_agent_chat_fork(&NewChatFork {
            thread_id: "fork",
            source: &source,
            sdk_session_id: "x",
            workspace_id: "ws",
            cwd: "/repo",
            title: "t",
            cut_before: None,
        })
        .unwrap();
        db.delete_agent_chat_session("src").unwrap();
        let origin = db.agent_chat_fork_origin("fork").unwrap();
        assert!(!origin.source_exists);
        assert_eq!(origin.source_title.as_deref(), Some("Fix the parser"));
        assert_eq!(origin.boundary_message_id, None, "nothing was copied");
    }
}
