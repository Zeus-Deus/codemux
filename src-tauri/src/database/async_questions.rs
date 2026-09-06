//! Durable question state and transcript updates share a transaction.
use super::*;
use crate::agent_provider::{ProviderRuntimeEvent, QuestionResolution, ThreadId, UserQuestionSet};

pub(super) fn create_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(r#"
        CREATE TABLE IF NOT EXISTS agent_chat_questions (
            thread_id TEXT NOT NULL REFERENCES agent_chat_sessions(thread_id) ON DELETE CASCADE,
            question_id TEXT NOT NULL,
            source_event_id INTEGER NOT NULL REFERENCES agent_chat_messages(id) ON DELETE CASCADE,
            question_json TEXT NOT NULL,
            resolution_json TEXT NOT NULL DEFAULT '{"status":"pending"}',
            PRIMARY KEY(thread_id, question_id)
        );
        CREATE TABLE IF NOT EXISTS agent_chat_question_revision (id INTEGER PRIMARY KEY CHECK(id=1), revision INTEGER NOT NULL);
        INSERT OR IGNORE INTO agent_chat_question_revision VALUES(1, 0);
        CREATE TRIGGER IF NOT EXISTS async_question_insert_revision AFTER INSERT ON agent_chat_questions
        BEGIN UPDATE agent_chat_question_revision SET revision=revision+1 WHERE id=1; END;
        CREATE TRIGGER IF NOT EXISTS async_question_update_revision AFTER UPDATE ON agent_chat_questions
        BEGIN UPDATE agent_chat_question_revision SET revision=revision+1 WHERE id=1; END;
        CREATE TRIGGER IF NOT EXISTS async_question_delete_revision AFTER DELETE ON agent_chat_questions
        BEGIN UPDATE agent_chat_question_revision SET revision=revision+1 WHERE id=1; END;
        CREATE TRIGGER IF NOT EXISTS async_question_thread_moved
        AFTER UPDATE OF thread_id ON agent_chat_messages
        WHEN json_extract(NEW.payload, '$.type') = 'questions_asked'
        BEGIN
            UPDATE OR IGNORE agent_chat_questions SET thread_id=NEW.thread_id WHERE source_event_id=NEW.id;
        END;
        CREATE TRIGGER IF NOT EXISTS async_question_resolution_deleted
        AFTER DELETE ON agent_chat_messages
        WHEN json_extract(OLD.payload, '$.type') = 'question_resolved'
        BEGIN
            -- A reverted answer must also remove its earlier dispatch claim,
            -- which can precede the checkpoint's pre-dispatch cutoff.
            DELETE FROM agent_chat_messages
            WHERE json_extract(OLD.payload, '$.resolution.status') = 'answered'
              AND thread_id = OLD.thread_id
              AND json_extract(payload, '$.type') = 'question_resolved'
              AND json_extract(payload, '$.question_id') = json_extract(OLD.payload, '$.question_id')
              AND json_extract(payload, '$.resolution.submission_id') = json_extract(OLD.payload, '$.resolution.submission_id')
              AND json_extract(payload, '$.resolution.status') IN ('submitting', 'unknown');
            UPDATE agent_chat_questions SET resolution_json = COALESCE((
                SELECT json_extract(payload, '$.resolution') FROM agent_chat_messages
                WHERE thread_id = OLD.thread_id
                  AND json_extract(payload, '$.type') = 'question_resolved'
                  AND json_extract(payload, '$.question_id') = json_extract(OLD.payload, '$.question_id')
                ORDER BY id DESC LIMIT 1
            ), '{"status":"pending"}')
            WHERE thread_id = OLD.thread_id AND question_id = json_extract(OLD.payload, '$.question_id');
        END;
    "#).map_err(|e| e.to_string())
}

fn append(conn: &Connection, thread_id: &str, event: &ProviderRuntimeEvent) -> Result<i64, String> {
    let payload = serde_json::to_string(event).map_err(|e| e.to_string())?;
    conn.execute("INSERT INTO agent_chat_messages(thread_id,payload,created_at) VALUES(?1,?2,strftime('%Y-%m-%d %H:%M:%f','now'))", params![thread_id,payload]).map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionAttention {
    pub revision: i64,
    pub workspaces: std::collections::HashMap<String, u64>,
}

impl DatabaseStore {
    pub fn async_question_attention(&self) -> Result<QuestionAttention, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let revision = conn
            .query_row(
                "SELECT revision FROM agent_chat_question_revision WHERE id=1",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let mut statement = conn.prepare("SELECT s.workspace_id,COUNT(*) FROM agent_chat_questions q JOIN agent_chat_sessions s ON s.thread_id=q.thread_id WHERE json_extract(q.resolution_json,'$.status') NOT IN ('answered','dismissed') GROUP BY s.workspace_id").map_err(|e|e.to_string())?;
        let workspaces = statement
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<rusqlite::Result<_>>()
            .map_err(|e| e.to_string())?;
        Ok(QuestionAttention {
            revision,
            workspaces,
        })
    }

    pub fn record_async_question(
        &self,
        thread_id: &str,
        question: &UserQuestionSet,
    ) -> Result<Option<i64>, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM agent_chat_questions WHERE thread_id=?1 AND question_id=?2)", params![thread_id, question.id], |r| r.get(0)).map_err(|e| e.to_string())?;
        if exists {
            return Ok(None);
        }
        let event = ProviderRuntimeEvent::QuestionsAsked {
            thread_id: ThreadId(thread_id.into()),
            question: question.clone(),
        };
        let id = append(&tx, thread_id, &event)?;
        tx.execute("INSERT INTO agent_chat_questions(thread_id,question_id,source_event_id,question_json) VALUES(?1,?2,?3,?4)", params![thread_id,question.id,id,serde_json::to_string(question).map_err(|e| e.to_string())?]).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(Some(id))
    }

    pub fn async_question(
        &self,
        thread_id: &str,
        id: &str,
    ) -> Result<(UserQuestionSet, QuestionResolution), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let (question,resolution): (String,String) = conn.query_row("SELECT question_json,resolution_json FROM agent_chat_questions WHERE thread_id=?1 AND question_id=?2", params![thread_id,id], |r| Ok((r.get(0)?,r.get(1)?))).map_err(|_| "Question no longer exists in this conversation.".to_string())?;
        Ok((
            serde_json::from_str(&question).map_err(|e| e.to_string())?,
            serde_json::from_str(&resolution).map_err(|e| e.to_string())?,
        ))
    }

    /// Compare-and-set prevents double submissions from concurrent windows.
    /// A changed record is persisted with its transcript event before fan-out.
    pub fn change_async_question(
        &self,
        thread_id: &str,
        id: &str,
        expected: &QuestionResolution,
        next: &QuestionResolution,
    ) -> Result<Option<i64>, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let updated = tx.execute("UPDATE agent_chat_questions SET resolution_json=?3 WHERE thread_id=?1 AND question_id=?2 AND resolution_json=?4", params![thread_id,id,serde_json::to_string(next).map_err(|e| e.to_string())?,serde_json::to_string(expected).map_err(|e| e.to_string())?]).map_err(|e| e.to_string())?;
        if updated == 0 {
            return Ok(None);
        }
        let event = ProviderRuntimeEvent::QuestionResolved {
            thread_id: ThreadId(thread_id.into()),
            question_id: id.into(),
            resolution: next.clone(),
        };
        let row = append(&tx, thread_id, &event)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(Some(row))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_provider::{QuestionDelivery, TurnId, UserQuestion};

    fn question() -> UserQuestionSet {
        UserQuestionSet {
            id: "q1".into(),
            target: "native".into(),
            source_item_id: "i1".into(),
            source_turn_id: "t1".into(),
            text: "Choose storage".into(),
            questions: vec![UserQuestion {
                title: "Which database?".into(),
                options: vec![],
            }],
            subagent_id: None,
        }
    }
    #[test]
    fn async_question_durable_claim_search_and_rollback() {
        let db = init_test_database();
        db.upsert_agent_chat_session("thread", "workspace", None, "codex")
            .unwrap();
        let q = question();
        let source = db.record_async_question("thread", &q).unwrap().unwrap();
        assert!(db.record_async_question("thread", &q).unwrap().is_none());
        assert_eq!(
            db.async_question_attention().unwrap().workspaces["workspace"],
            1
        );
        let claimed = QuestionResolution::Submitting {
            submission_id: "s1".into(),
            answers: vec!["PostgreSQL".into()],
        };
        let claim_row = db
            .change_async_question("thread", &q.id, &QuestionResolution::Pending, &claimed)
            .unwrap()
            .unwrap();
        assert!(db
            .change_async_question("thread", &q.id, &QuestionResolution::Pending, &claimed)
            .unwrap()
            .is_none());
        let answered = QuestionResolution::Answered {
            submission_id: "s1".into(),
            answers: vec!["PostgreSQL".into()],
            delivery: QuestionDelivery::Inflight {
                turn_id: TurnId("t1".into()),
            },
        };
        let reply = db
            .change_async_question("thread", &q.id, &claimed, &answered)
            .unwrap()
            .unwrap();
        assert!(db.async_question_attention().unwrap().workspaces.is_empty());
        let conn = db.conn.lock().unwrap();
        super::super::create_schema(&conn).unwrap(); // reopening retains the durable projection
        let hits: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_chat_search WHERE agent_chat_search MATCH 'PostgreSQL'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hits, 1);
        let source_hit: i64 = conn
            .query_row(
                "SELECT rowid FROM agent_chat_search WHERE agent_chat_search MATCH 'database'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(source_hit, source);
        conn.execute("DELETE FROM agent_chat_messages WHERE id = ?1", [reply])
            .unwrap();
        assert!(reply > claim_row);
        drop(conn);
        assert_eq!(
            db.async_question("thread", &q.id).unwrap().1,
            QuestionResolution::Pending
        );
        let conn = db.conn.lock().unwrap();
        conn.execute("DELETE FROM agent_chat_messages WHERE id=?1", [source])
            .unwrap();
        drop(conn);
        assert!(db.async_question("thread", &q.id).is_err());
    }
    #[test]
    fn async_question_follows_session_promotion_and_deletion() {
        let db = init_test_database();
        for id in ["old", "new"] {
            db.upsert_agent_chat_session(id, "workspace", None, "codex")
                .unwrap();
        }
        db.record_async_question("old", &question()).unwrap();
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "UPDATE agent_chat_messages SET thread_id='new' WHERE thread_id='old'",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM agent_chat_sessions WHERE thread_id='old'", [])
            .unwrap();
        drop(conn);
        assert_eq!(db.async_question("new", "q1").unwrap().0.target, "native");
        let conn = db.conn.lock().unwrap();
        conn.execute("DELETE FROM agent_chat_sessions WHERE thread_id='new'", [])
            .unwrap();
        drop(conn);
        assert!(db.async_question_attention().unwrap().workspaces.is_empty());
    }
}
