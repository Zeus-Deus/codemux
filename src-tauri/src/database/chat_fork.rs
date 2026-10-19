//! Chat forks (`commands::chat_fork`).
//!
//! A fork is an ordinary chat thread: its own `agent_chat_sessions` row, its
//! own copy of the source transcript up to the fork point, and the provider's
//! forked session as its resume cursor. This table only records lineage, so
//! the fork can say where it came from. The row cascades with the fork; it
//! deliberately has no foreign key to the source, which may be deleted
//! while the fork lives on. Reopening either from history keeps the link
//! ([`move_to_thread`]).
use super::*;
use std::path::Path;

pub(super) fn create_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_chat_forks (
            thread_id TEXT PRIMARY KEY,
            source_thread_id TEXT NOT NULL,
            source_title TEXT,
            -- The source row the fork's copy stopped before.
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

/// A history reopen merges `merged` into `survivor`
/// (`merge_agent_chat_session_rows`): the survivor keeps the lineage of the
/// fork it continues, unless it has its own, and forks of a merged chat now
/// come from the survivor.
pub(super) fn move_to_thread(conn: &Connection, merged: &[String], survivor: &str) -> Result<(), String> {
    let move_error = |e: rusqlite::Error| format!("Failed to keep the fork's origin: {e}");
    for thread in merged {
        conn.execute(
            "UPDATE OR IGNORE agent_chat_forks SET thread_id = ?2 WHERE thread_id = ?1",
            params![thread, survivor],
        )
        .map_err(move_error)?;
        conn.execute(
            "UPDATE agent_chat_forks SET source_thread_id = ?2
             WHERE source_thread_id = ?1 AND thread_id != ?2",
            params![thread, survivor],
        )
        .map_err(move_error)?;
    }
    Ok(())
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
    /// The source's and the fork's chat-image folders. Inherited images are
    /// re-pointed into the fork's own folder, so deleting or reverting the
    /// source can't break them.
    pub image_dirs: Option<(&'a Path, &'a Path)>,
}

/// An inherited image the fork's transcript now points at: copy `from` to
/// `to` once the fork is written.
pub type ForkedImage = (PathBuf, PathBuf);

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
/// `thread_id` names the fork, a user-turn journal row points at the fork's
/// copy of its user message, and a user message's images inside the
/// source's image folder move to the fork's (recorded in `images`).
pub(crate) fn rewrite_forked_payload(
    payload: &str,
    fork_thread_id: &str,
    copied_ids: &HashMap<i64, i64>,
    image_dirs: Option<(&Path, &Path)>,
    images: &mut Vec<ForkedImage>,
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
    if let (Some((source_dir, fork_dir)), Some(list)) = (
        image_dirs,
        object.get_mut("images").and_then(serde_json::Value::as_array_mut),
    ) {
        for image in list {
            let Some(path) = image.get("path").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let from = PathBuf::from(path);
            let Ok(rest) = from.strip_prefix(source_dir) else {
                continue;
            };
            let to = fork_dir.join(rest);
            image["path"] = to.to_string_lossy().into_owned().into();
            images.push((from, to));
        }
    }
    value.to_string()
}

/// Carry the source's async questions into the fork as they stood at the
/// cut, so an inherited question stays answerable. Every one routes to the
/// fork's own native thread: the source's threads, its subagents included,
/// aren't the fork's to steer, so an answer lands in the fork's conversation.
fn copy_forked_questions(
    tx: &rusqlite::Transaction<'_>,
    source_thread_id: &str,
    fork: &NewChatFork<'_>,
    copied_ids: &HashMap<i64, i64>,
) -> Result<(), String> {
    let questions: Vec<(String, i64, String)> = {
        let mut stmt = tx
            .prepare(
                "SELECT question_id, source_event_id, question_json
                 FROM agent_chat_questions WHERE thread_id = ?1",
            )
            .map_err(|e| e.to_string())?;
        let mapped = stmt
            .query_map(params![source_thread_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(|e| e.to_string())?;
        mapped.collect::<Result<_, _>>().map_err(|e| e.to_string())?
    };
    for (question_id, source_event, question_json) in questions {
        // Asked after the cut: not part of the fork.
        let Some(&event) = copied_ids.get(&source_event) else {
            continue;
        };
        let mut question: serde_json::Value =
            serde_json::from_str(&question_json).map_err(|e| e.to_string())?;
        question["target"] = fork.sdk_session_id.into();
        // The newest resolution the fork kept, as the resolution trigger
        // in `async_questions` recomputes it.
        let resolution: Option<String> = tx
            .query_row(
                "SELECT json_extract(payload, '$.resolution') FROM agent_chat_messages
                 WHERE thread_id = ?1 AND json_valid(payload)
                   AND json_extract(payload, '$.type') = 'question_resolved'
                   AND json_extract(payload, '$.question_id') = ?2
                 ORDER BY id DESC LIMIT 1",
                params![fork.thread_id, question_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO agent_chat_questions
                 (thread_id, question_id, source_event_id, question_json, resolution_json)
             VALUES (?1, ?2, ?3, ?4, COALESCE(?5, '{\"status\":\"pending\"}'))",
            params![fork.thread_id, question_id, event, question.to_string(), resolution],
        )
        .map_err(|e| format!("Failed to copy the chat's questions: {e}"))?;
    }
    Ok(())
}

impl DatabaseStore {
    /// Create the fork thread in one transaction: its session row, its copy
    /// of the source transcript, and its lineage row. Fails if the thread id
    /// is already taken, so a retried fork never merges into another chat.
    /// Returns the inherited images the caller copies into the fork's folder.
    pub fn create_agent_chat_fork(
        &self,
        fork: &NewChatFork<'_>,
    ) -> Result<Vec<ForkedImage>, String> {
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
                    // The source's queue journal stays behind: the fork has
                    // no queue, and an admission whose dispatch falls after
                    // the cut would read as a message still waiting to send.
                    "SELECT id, payload, created_at FROM agent_chat_messages
                     WHERE thread_id = ?1 AND (?2 IS NULL OR id < ?2)
                       AND NOT (json_valid(payload)
                                AND json_extract(payload, '$.type') = 'native_queue_control')
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
        let mut images = Vec::new();
        for (id, payload, created_at) in rows {
            let payload = rewrite_forked_payload(
                &payload,
                fork.thread_id,
                &copied_ids,
                fork.image_dirs,
                &mut images,
            );
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
        copy_forked_questions(&tx, source.thread_id.as_str(), fork, &copied_ids)?;
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
        tx.commit().map_err(|e| e.to_string())?;
        Ok(images)
    }

    /// The newest row and the newest user message of a thread.
    pub fn agent_chat_fork_heads(&self, thread_id: &str) -> (Option<i64>, Option<i64>) {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT MAX(id),
                        MAX(CASE WHEN json_valid(payload)
                                  AND json_extract(payload, '$.type') = 'user_message'
                             THEN id END)
                 FROM agent_chat_messages WHERE thread_id = ?1",
                params![thread_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or((None, None))
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

    /// What a Claude cut before row `before` needs: the text of every user
    /// message in the thread, in order (each clipped to
    /// [`FORK_PROMPT_CHARS`]), and the position of the one at `before`,
    /// which must start its own turn.
    pub fn agent_chat_fork_prompts(
        &self,
        thread_id: &str,
        before: i64,
    ) -> Result<(Vec<String>, usize), String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, payload FROM agent_chat_messages
                 WHERE thread_id = ?1
                   AND json_valid(payload) AND json_extract(payload, '$.type') = 'user_message'
                 ORDER BY id ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![thread_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let mut prompts = Vec::with_capacity(rows.len());
        let mut cut = None;
        for (id, payload) in rows {
            let value: serde_json::Value = serde_json::from_str(&payload).unwrap_or_default();
            if id == before && value.get("steered_turn_id").is_none() {
                cut = Some(prompts.len());
            }
            let text = value.get("text").and_then(serde_json::Value::as_str).unwrap_or("");
            prompts.push(text.chars().take(FORK_PROMPT_CHARS).collect());
        }
        let cut = cut.ok_or("That message doesn't start a turn, so the chat can't be forked there.")?;
        Ok((prompts, cut))
    }
}

/// How much of each prompt a Claude cut compares: enough to tell turns
/// apart without shipping whole pasted documents to the sidecar.
pub const FORK_PROMPT_CHARS: usize = 500;

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
        let mut images = Vec::new();
        let row = rewrite_forked_payload(
            r#"{"type":"native_user_control","thread_id":"src","user_message_id":7}"#,
            "fork",
            &ids,
            None,
            &mut images,
        );
        let row: serde_json::Value = serde_json::from_str(&row).unwrap();
        assert_eq!(row["thread_id"], "fork");
        assert_eq!(row["user_message_id"], 70);
        assert_eq!(rewrite_forked_payload("not json", "fork", &ids, None, &mut images), "not json");
        let untouched =
            rewrite_forked_payload(r#"{"type":"turn_completed"}"#, "fork", &ids, None, &mut images);
        assert_eq!(untouched, r#"{"type":"turn_completed"}"#);
        assert!(images.is_empty());
    }

    #[test]
    fn rewrite_moves_inherited_images_into_the_forks_folder() {
        let mut images = Vec::new();
        let row = rewrite_forked_payload(
            r#"{"type":"user_message","text":"look","images":[{"path":"/img/src/a.png","media_type":"image/png"},{"path":"/elsewhere/b.png"}]}"#,
            "fork",
            &HashMap::new(),
            Some((Path::new("/img/src"), Path::new("/img/fork"))),
            &mut images,
        );
        let row: serde_json::Value = serde_json::from_str(&row).unwrap();
        let moved = Path::new("/img/fork").join("a.png");
        assert_eq!(row["images"][0]["path"], moved.to_string_lossy().as_ref());
        assert_eq!(row["images"][1]["path"], "/elsewhere/b.png", "only the source's own folder moves");
        assert_eq!(images, vec![(PathBuf::from("/img/src/a.png"), PathBuf::from("/img/fork/a.png"))]);
    }

    #[test]
    fn fork_copies_the_transcript_up_to_the_cut_and_records_lineage() {
        let db = store_with_source();
        append(&db, serde_json::json!({"type":"user_message","thread_id":"src","text":"one"}));
        append(&db, serde_json::json!({"type":"turn_completed","thread_id":"src"}));
        append(&db, serde_json::json!({"type":"native_queue_control","thread_id":"src","queued_id":"q","disposition":"accepted"}));
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
            image_dirs: None,
        })
        .unwrap();

        let copied = db.list_agent_chat_messages("fork");
        assert_eq!(copied.len(), 2, "rows from the cut on, and the queue journal, stay behind");
        assert!(db.control_unresolved_queued_ids("fork").unwrap().is_empty());
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
        assert_eq!(db.list_agent_chat_messages("src").len(), 5, "the source is untouched");
        assert!(db.agent_chat_fork_origin("src").is_none());
    }

    #[test]
    fn fork_keeps_questions_as_they_stood_at_the_cut_and_answers_them_itself() {
        use crate::agent_provider::{QuestionResolution, UserQuestion, UserQuestionSet};
        let db = store_with_source();
        let question = |id: &str, target: &str| UserQuestionSet {
            id: id.into(),
            target: target.into(),
            source_item_id: "item".into(),
            source_turn_id: "turn".into(),
            text: "Which one?".into(),
            questions: vec![UserQuestion { title: "Pick".into(), options: vec![] }],
            subagent_id: None,
        };
        db.record_async_question("src", &question("open", "native-src")).unwrap();
        db.record_async_question("src", &question("dismissed", "native-child")).unwrap();
        db.change_async_question("src", "dismissed", &QuestionResolution::Pending, &QuestionResolution::Dismissed)
            .unwrap()
            .unwrap();
        let cut = append(&db, serde_json::json!({"type":"user_message","thread_id":"src","text":"two"}));
        // Dismissed in the source only after the cut, and asked after it.
        db.change_async_question("src", "open", &QuestionResolution::Pending, &QuestionResolution::Dismissed)
            .unwrap()
            .unwrap();
        db.record_async_question("src", &question("later", "native-src")).unwrap();
        let source = db.get_agent_chat_session("src").unwrap();
        db.create_agent_chat_fork(&NewChatFork {
            thread_id: "fork",
            source: &source,
            sdk_session_id: "native-fork",
            workspace_id: "ws",
            cwd: "/repo",
            title: "t",
            cut_before: Some(cut),
            image_dirs: None,
        })
        .unwrap();

        let (open, resolution) = db.async_question("fork", "open").unwrap();
        assert_eq!(resolution, QuestionResolution::Pending, "as it stood at the cut");
        assert_eq!(open.target, "native-fork");
        let (dismissed, resolution) = db.async_question("fork", "dismissed").unwrap();
        assert_eq!(resolution, QuestionResolution::Dismissed);
        assert_eq!(dismissed.target, "native-fork", "never steers the source's threads");
        assert!(db.async_question("fork", "later").is_err());
        assert_eq!(db.async_question("src", "open").unwrap().1, QuestionResolution::Dismissed, "the source is untouched");
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
            image_dirs: None,
        };
        assert!(db.create_agent_chat_fork(&fork).is_err());
        assert!(db.agent_chat_fork_origin("src").is_none(), "nothing half-written");
    }

    #[test]
    fn fork_prompts_list_every_user_message_and_place_the_cut() {
        let db = store_with_source();
        append(&db, serde_json::json!({"type":"user_message","text":"one"}));
        let reply = append(&db, serde_json::json!({"type":"turn_completed"}));
        let steered = append(&db, serde_json::json!({"type":"user_message","text":"also","steered_turn_id":"t1"}));
        let next = append(&db, serde_json::json!({"type":"user_message","text":"two"}));
        append(&db, serde_json::json!({"type":"user_message","text":"x".repeat(FORK_PROMPT_CHARS + 9)}));
        let (prompts, cut) = db.agent_chat_fork_prompts("src", next).unwrap();
        assert_eq!(&prompts[..3], ["one", "also", "two"]);
        assert_eq!(prompts[3].chars().count(), FORK_PROMPT_CHARS);
        assert_eq!(cut, 2);
        assert!(db.agent_chat_fork_prompts("src", reply).is_err());
        assert!(db.agent_chat_fork_prompts("src", steered).is_err(), "a steer is not a turn");
        assert!(db.agent_chat_fork_prompts("other", next).is_err());
    }

    #[test]
    fn heads_track_the_newest_row_and_user_message() {
        let db = store_with_source();
        assert_eq!(db.agent_chat_fork_heads("src"), (None, None));
        let user = append(&db, serde_json::json!({"type":"user_message","text":"one"}));
        let done = append(&db, serde_json::json!({"type":"turn_completed"}));
        assert_eq!(db.agent_chat_fork_heads("src"), (Some(done), Some(user)));
    }

    #[test]
    fn reopening_a_fork_or_its_source_from_history_keeps_the_lineage() {
        let db = store_with_source();
        append(&db, serde_json::json!({"type":"user_message","thread_id":"src","text":"one"}));
        let source = db.get_agent_chat_session("src").unwrap();
        db.create_agent_chat_fork(&NewChatFork {
            thread_id: "fork",
            source: &source,
            sdk_session_id: "native-fork",
            workspace_id: "ws",
            cwd: "/repo",
            title: "Fix the parser (fork)",
            cut_before: None,
            image_dirs: None,
        })
        .unwrap();
        let boundary = db.agent_chat_fork_origin("fork").unwrap().boundary_message_id;
        db.set_agent_chat_sdk_session_id("src", "native-src").unwrap();
        // Each is reopened from history under a fresh thread id.
        for (thread, sdk) in [("src2", "native-src"), ("fork2", "native-fork")] {
            db.upsert_agent_chat_session(thread, "ws", Some("/repo"), "claude").unwrap();
            db.conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE agent_chat_sessions SET sdk_session_id = ?2,
                         last_active_at = datetime('now', '+1 day') WHERE thread_id = ?1",
                    params![thread, sdk],
                )
                .unwrap();
            db.collapse_duplicate_agent_chat_sessions(sdk).unwrap();
        }

        assert!(db.agent_chat_fork_origin("fork").is_none());
        let origin = db.agent_chat_fork_origin("fork2").unwrap();
        assert_eq!(origin.source_thread_id, "src2");
        assert!(origin.source_exists);
        assert_eq!(origin.source_title.as_deref(), Some("Fix the parser"));
        assert_eq!(origin.boundary_message_id, boundary);
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
            image_dirs: None,
        })
        .unwrap();
        db.delete_agent_chat_session("src").unwrap();
        let origin = db.agent_chat_fork_origin("fork").unwrap();
        assert!(!origin.source_exists);
        assert_eq!(origin.source_title.as_deref(), Some("Fix the parser"));
        assert_eq!(origin.boundary_message_id, None, "nothing was copied");
    }
}
