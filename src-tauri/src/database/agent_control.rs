//! Durable admission receipts. Request payloads are hashed, not stored.
use super::DatabaseStore;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS agent_control_operations (
    id TEXT PRIMARY KEY,
    principal TEXT NOT NULL,
    request_key TEXT NOT NULL,
    tool TEXT NOT NULL,
    payload_hash TEXT NOT NULL,
    epoch TEXT NOT NULL,
    state TEXT NOT NULL,
    workspace_id TEXT,
    thread_id TEXT,
    result TEXT,
    error TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(principal, request_key)
);
CREATE INDEX IF NOT EXISTS agent_control_operations_owner
ON agent_control_operations(principal, created_at);
";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationReceipt {
    pub operation_id: String,
    pub tool: String,
    pub state: String,
    pub workspace_id: Option<String>,
    pub thread_id: Option<String>,
    pub result: Option<Value>,
    pub error: Option<Value>,
    pub created_at: String,
    pub updated_at: String,
}

pub struct Admission {
    pub receipt: OperationReceipt,
    pub newly_admitted: bool,
}

impl DatabaseStore {
    /// Snapshot real bridge-written SQLite state at a deterministic crash
    /// boundary. Restart reads open the file, never re-seed event fixtures.
    #[cfg(test)]
    pub(crate) fn control_fixture_snapshot(&self, path: &std::path::Path) -> Self {
        self.conn.lock().unwrap().execute("VACUUM INTO ?1", params![path.to_str().unwrap()]).unwrap();
        Self { conn: std::sync::Mutex::new(super::open_connection(path).unwrap()) }
    }

    /// Durable control-only queue evidence. It intentionally is NOT a
    /// ProviderRuntimeEvent: replay cannot restore a queued bubble or callback.
    /// Only IDs/dispositions are stored, never prompt text or tool payloads.
    fn control_record_queue_transition(&self, thread: &str, queued: &str, disposition: &str,
        turn: Option<&str>, steered: bool) -> Result<(), String> {
        let payload = serde_json::json!({"type":"native_queue_control","thread_id":thread,
            "queued_id":queued,"disposition":disposition,"turn_id":turn,"steered":steered});
        self.conn.lock().unwrap().execute(
            "INSERT INTO agent_chat_messages (thread_id,payload,created_at)
             SELECT ?1,?2,strftime('%Y-%m-%d %H:%M:%f','now')
             WHERE NOT EXISTS (SELECT 1 FROM agent_chat_messages
                 WHERE thread_id=?1 AND json_extract(payload,'$.type')='native_queue_control'
                   AND json_extract(payload,'$.queued_id')=?3 AND json_extract(payload,'$.disposition')=?4)",
            params![thread,payload.to_string(),queued,disposition],
        ).map_err(|e|format!("queue_journal_failed: {e}"))?;
        Ok(())
    }

    /// The RPC acknowledgement and event bridge race. Admission deduplicates,
    /// while any actual disposition wins even if its event preceded the ACK.
    pub(crate) fn control_record_queue_admission(&self, thread: &str, queued: &str) -> Result<(), String> {
        self.control_record_queue_transition(thread,queued,"accepted",None,false)
    }

    pub(crate) fn control_record_queue_event(&self, event: &crate::agent_provider::ProviderRuntimeEvent) -> Result<(), String> {
        use crate::agent_provider::ProviderRuntimeEvent;
        match event {
            ProviderRuntimeEvent::TurnQueued {thread_id,queued_id,..} => self.control_record_queue_admission(&thread_id.0,queued_id),
            ProviderRuntimeEvent::QueuedTurnDispatched {thread_id,queued_id,turn_id,steered,..} =>
                self.control_record_queue_transition(&thread_id.0,queued_id,"dispatched",Some(&turn_id.0),*steered),
            ProviderRuntimeEvent::QueuedTurnCancelled {thread_id,queued_id} =>
                self.control_record_queue_transition(&thread_id.0,queued_id,"cancelled",None,false),
            _ => Ok(()),
        }
    }

    /// Historical uncertainty is read-only evidence, never native queued_ids.
    pub(crate) fn control_unresolved_queued_ids(&self, thread: &str) -> Result<Vec<String>, String> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT json_extract(a.payload,'$.queued_id') FROM agent_chat_messages a
             WHERE a.thread_id=?1 AND json_extract(a.payload,'$.type')='native_queue_control'
               AND json_extract(a.payload,'$.disposition')='accepted'
               AND NOT EXISTS (SELECT 1 FROM agent_chat_messages d
                   WHERE d.thread_id=a.thread_id AND json_extract(d.payload,'$.type')='native_queue_control'
                     AND json_extract(d.payload,'$.queued_id')=json_extract(a.payload,'$.queued_id')
                     AND json_extract(d.payload,'$.disposition') IN ('dispatched','cancelled'))
             ORDER BY a.id ASC",
        ).map_err(|e|e.to_string())?;
        let rows = statement.query_map(params![thread], |row|row.get(0)).map_err(|e|e.to_string())?;
        rows.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())
    }

    pub fn control_operation_by_key(
        &self, principal: &str, key: &str, tool: &str, hash: &str, epoch: &str,
    ) -> Result<Option<OperationReceipt>, String> {
        let existing: Option<(String, String, String)> = self.conn.lock().unwrap().query_row(
            "SELECT id, tool, payload_hash FROM agent_control_operations WHERE principal=?1 AND request_key=?2",
            params![principal, key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).optional().map_err(|e| e.to_string())?;
        match existing {
            Some((id, admitted_tool, admitted_hash)) => {
                if admitted_tool != tool || admitted_hash != hash {
                    return Err("request_key_conflict: retry arguments must be unchanged".into());
                }
                self.control_operation(principal, &id, epoch)
            }
            None => Ok(None),
        }
    }

    pub fn control_turn_outcome(&self, thread: &str, turn: &str) -> Result<Option<Value>, String> {
        let event: Option<String> = self.conn.lock().unwrap().query_row(
            "SELECT payload FROM agent_chat_messages WHERE thread_id=?1
             AND json_extract(payload,'$.type')='turn_completed' AND json_extract(payload,'$.turn_id')=?2
             ORDER BY id DESC LIMIT 1",
            params![thread,turn], |row|row.get(0),
        ).optional().map_err(|e|e.to_string())?;
        event.map(|event|serde_json::from_str(&event).map_err(|e|e.to_string())).transpose()
    }

    /// Append the unchanged visible envelope and its ID-only run correlation
    /// atomically. A crash must not leave a late envelope without its identity.
    /// Neither this journal nor historical IDs restore runtime authority.
    pub(crate) fn control_append_user_message(&self, thread: &str, payload: &str, turn: &str, steered: bool) -> Result<Option<i64>, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e|e.to_string())?;
        match tx.execute(
            "INSERT INTO agent_chat_messages (thread_id,payload,created_at)
             VALUES (?1,?2,strftime('%Y-%m-%d %H:%M:%f','now'))",
            params![thread,payload],
        ) {
            Ok(_) => {},
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY => return Ok(None),
            Err(e) => return Err(e.to_string()),
        }
        let row = tx.last_insert_rowid();
        if !turn.is_empty() {
            let identity = serde_json::json!({"type":"native_user_control","thread_id":thread,
                "user_message_id":row,"turn_id":turn,"steered":steered});
            tx.execute(
                "INSERT INTO agent_chat_messages (thread_id,payload,created_at)
                 VALUES (?1,?2,strftime('%Y-%m-%d %H:%M:%f','now'))",
                params![thread,identity.to_string()],
            ).map_err(|e|format!("user_turn_journal_failed: {e}"))?;
        }
        tx.commit().map_err(|e|e.to_string())?;
        Ok(Some(row))
    }

    /// Order logical runs by their first durable evidence, not a late ACK.
    /// Legacy uncorrelated user rows remain unknown; never guess their IDs.
    pub fn control_last_thread_run_event(&self, thread: &str) -> Result<Option<Value>, String> {
        let event: Option<String> = self.conn.lock().unwrap().query_row(
            "WITH evidence AS (
                 SELECT id,payload,
                    CASE WHEN json_extract(payload,'$.type') IN ('user_message','turn_queued')
                         OR COALESCE(json_extract(payload,'$.turn_id'),'')=''
                         THEN 'row:'||id ELSE 'turn:'||json_extract(payload,'$.turn_id') END AS run,
                    CASE WHEN json_extract(payload,'$.type')='native_user_control'
                         THEN json_extract(payload,'$.user_message_id') ELSE id END AS position
                 FROM agent_chat_messages a WHERE thread_id=?1 AND (
                    json_extract(payload,'$.type') IN ('turn_completed','turn_queued')
                    OR (json_extract(payload,'$.type')='queued_turn_dispatched'
                        AND COALESCE(json_extract(payload,'$.steered'),0)=0)
                    OR (json_extract(payload,'$.type')='native_queue_control'
                        AND json_extract(payload,'$.disposition')='dispatched' AND json_extract(payload,'$.steered')=0)
                    OR (json_extract(payload,'$.type')='native_user_control' AND json_extract(payload,'$.steered')=0)
                    OR (json_extract(payload,'$.type')='user_message'
                        AND COALESCE(json_extract(payload,'$.steered_turn_id'),'')=''
                        AND NOT EXISTS (SELECT 1 FROM agent_chat_messages b WHERE b.thread_id=a.thread_id
                            AND json_valid(b.payload, 2) AND json_extract(b.payload,'$.type')='native_user_control'
                            AND CAST(json_extract(b.payload,'$.user_message_id') AS NUMERIC)=a.id
                            AND json_extract(b.payload,'$.user_message_id')=a.id))
                 )
             ), runs AS (SELECT run,MIN(position) AS first_position FROM evidence GROUP BY run)
             SELECT payload FROM evidence JOIN runs USING(run)
               ORDER BY first_position DESC, json_extract(payload,'$.type')='turn_completed' DESC, id DESC LIMIT 1",
            params![thread], |row|row.get(0),
        ).optional().map_err(|e|e.to_string())?;
        event.map(|event| serde_json::from_str(&event).map_err(|e|e.to_string())).transpose()
    }

    /// Unlike resume pickers, discovery includes empty and cursorless threads.
    pub fn control_thread_ids(&self, workspace: &str, cursor: Option<&str>, limit: u32) -> Result<Vec<String>, String> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare(
            "SELECT thread_id FROM agent_chat_sessions WHERE workspace_id=?1
             AND (?2 IS NULL OR thread_id>?2) ORDER BY thread_id ASC LIMIT ?3",
        ).map_err(|e| e.to_string())?;
        let rows = statement.query_map(params![workspace,cursor,limit], |row|row.get(0)).map_err(|e|e.to_string())?;
        rows.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())
    }

    pub fn update_control_operation(
        &self, principal: &str, id: &str, epoch: &str, state: &str,
        workspace_id: Option<&str>, thread_id: Option<&str>,
        result: Option<&Value>, error: Option<&Value>,
    ) -> Result<(), String> {
        if !matches!(state, "running" | "succeeded" | "failed" | "needs_attention") {
            return Err("invalid_operation_state".into());
        }
        let changed = self.conn.lock().unwrap().execute(
            "UPDATE agent_control_operations
             SET state=?4, workspace_id=COALESCE(?5,workspace_id), thread_id=COALESCE(?6,thread_id),
                 result=?7, error=?8, updated_at=datetime('now')
             WHERE principal=?1 AND id=?2 AND epoch=?3 AND state IN ('accepted','running')",
            params![principal,id,epoch,state,workspace_id,thread_id,
                result.map(Value::to_string),error.map(Value::to_string)],
        ).map_err(|e| e.to_string())?;
        if changed == 0 { return Err("operation_no_longer_pending".into()); }
        Ok(())
    }

    pub fn admit_control_operation(
        &self, principal: &str, request_key: &str, tool: &str, payload_hash: &str,
        epoch: &str, workspace_id: Option<&str>, thread_id: Option<&str>,
    ) -> Result<Admission, String> {
        let conn = self.conn.lock().unwrap();
        let existing: Option<(String, String, String)> = conn.query_row(
            "SELECT id, tool, payload_hash FROM agent_control_operations
             WHERE principal=?1 AND request_key=?2",
            params![principal, request_key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).optional().map_err(|e| e.to_string())?;
        if let Some((id, admitted_tool, admitted_hash)) = existing {
            if admitted_tool != tool || admitted_hash != payload_hash {
                return Err("request_key_conflict: retry arguments must be unchanged".into());
            }
            drop(conn);
            return Ok(Admission {
                receipt: self.control_operation(principal, &id, epoch)?.ok_or("operation_not_found")?,
                newly_admitted: false,
            });
        }
        let id = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO agent_control_operations
             (id, principal, request_key, tool, payload_hash, epoch, state, workspace_id, thread_id)
             VALUES (?1,?2,?3,?4,?5,?6,'accepted',?7,?8)",
            params![id, principal, request_key, tool, payload_hash, epoch, workspace_id, thread_id],
        ).map_err(|e| e.to_string())?;
        drop(conn);
        Ok(Admission { receipt: self.control_operation(principal, &id, epoch)?.ok_or("operation_not_found")?, newly_admitted: true })
    }

    pub fn control_operation(&self, principal: &str, id: &str, epoch: &str) -> Result<Option<OperationReceipt>, String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agent_control_operations SET state='needs_attention', updated_at=datetime('now')
             WHERE principal=?1 AND id=?2 AND epoch<>?3 AND state IN ('accepted','running')",
            params![principal, id, epoch],
        ).map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT id, tool, state, workspace_id, thread_id, result, error, created_at, updated_at
             FROM agent_control_operations WHERE principal=?1 AND id=?2",
            params![principal,id], |row| {
                let result: Option<String> = row.get(5)?;
                let error: Option<String> = row.get(6)?;
                Ok(OperationReceipt {
                    operation_id: row.get(0)?, tool: row.get(1)?, state: row.get(2)?,
                    workspace_id: row.get(3)?, thread_id: row.get(4)?,
                    result: result.and_then(|v| serde_json::from_str(&v).ok()),
                    error: error.and_then(|v| serde_json::from_str(&v).ok()),
                    created_at: row.get(7)?, updated_at: row.get(8)?,
                })
            },
        ).optional().map_err(|e|e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGINAL_QUERY: &str = r#"WITH evidence AS (
                 SELECT id,payload,
                    CASE WHEN json_extract(payload,'$.type') IN ('user_message','turn_queued')
                         OR COALESCE(json_extract(payload,'$.turn_id'),'')=''
                         THEN 'row:'||id ELSE 'turn:'||json_extract(payload,'$.turn_id') END AS run,
                    CASE WHEN json_extract(payload,'$.type')='native_user_control'
                         THEN json_extract(payload,'$.user_message_id') ELSE id END AS position
                 FROM agent_chat_messages a WHERE thread_id=?1 AND (
                    json_extract(payload,'$.type') IN ('turn_completed','turn_queued')
                    OR (json_extract(payload,'$.type')='queued_turn_dispatched'
                        AND COALESCE(json_extract(payload,'$.steered'),0)=0)
                    OR (json_extract(payload,'$.type')='native_queue_control'
                        AND json_extract(payload,'$.disposition')='dispatched' AND json_extract(payload,'$.steered')=0)
                    OR (json_extract(payload,'$.type')='native_user_control' AND json_extract(payload,'$.steered')=0)
                    OR (json_extract(payload,'$.type')='user_message'
                        AND COALESCE(json_extract(payload,'$.steered_turn_id'),'')=''
                        AND NOT EXISTS (SELECT 1 FROM agent_chat_messages b WHERE b.thread_id=a.thread_id
                            AND json_extract(b.payload,'$.type')='native_user_control'
                            AND json_extract(b.payload,'$.user_message_id')=a.id))
                 )
             ), runs AS (SELECT run,MIN(position) AS first_position FROM evidence GROUP BY run)
             SELECT payload FROM evidence JOIN runs USING(run)
               ORDER BY first_position DESC, json_extract(payload,'$.type')='turn_completed' DESC, id DESC LIMIT 1"#;

    fn json5_capture(value: Value) {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _capture_guard = LOCK.lock().unwrap();
        if let Ok(path) = std::env::var("CODEMUX_JSON5_CAPTURE") {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .unwrap();
            writeln!(file, "{value}").unwrap();
        }
    }

    fn original_last_run(db: &DatabaseStore, thread: &str) -> Result<Option<Value>, String> {
        let event: Option<String> = db
            .conn
            .lock()
            .unwrap()
            .query_row(ORIGINAL_QUERY, [thread], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?;
        event
            .map(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
            .transpose()
    }

    fn json5_rows(db: &DatabaseStore, thread: &str) -> Vec<(i64, String, String)> {
        let conn = db.conn.lock().unwrap();
        let rows = conn.prepare("SELECT id,payload,typeof(payload) FROM agent_chat_messages WHERE thread_id=?1 ORDER BY id").unwrap()
            .query_map([thread], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap()
            .collect::<rusqlite::Result<Vec<_>>>().unwrap();
        rows
    }

    // These are intentionally noncanonical TEXT specimens, not repairs of JSON.
    fn json5_journals() -> Vec<&'static str> {
        vec![
            r#"{"type":"native_user_control","user_message_id":2,"turn_id":"a","steered":false,}"#,
            r#"{type:'native_user_control',user_message_id:2,turn_id:'a',steered:false}"#,
            r#"{/* correlation */ "type":"native_user_control","user_message_id":+2,"turn_id":"a","steered":false}"#,
            r#"{"type":"native_user_control","user_message_id":0x2,"turn_id":"a","steered":false}"#,
            r#"{"type":"native_user_control","user_message_id":2.,"turn_id":"a","steered":false}"#,
            r#"{"type":"native_user_control","user_message_id":2,"turn_id":"a","steered":false,"extra":Infinity}"#,
            r#"{"type":"native_user_control","user_message_id":2,"turn_id":"a","steered":false,"extra":NaN}"#,
            r#"{"type":"native_user_control","user_message_id":2,"turn_id":"a","steered":false,"extra":QNaN}"#,
            r#"{"type":"native_user_control","user_message_id":2,"turn_id":"a","steered":false,"extra":Inf}"#,
        ]
    }

    fn json5_fixture(journal: &str) -> DatabaseStore {
        let db = DatabaseStore::new_in_memory();
        db.upsert_agent_chat_session("t", "w", None, "opencode")
            .unwrap();
        db.append_agent_chat_message(
            "t",
            r#"{"type":"turn_completed","turn_id":"a","status":{"kind":"success"}}"#,
        )
        .unwrap();
        assert_eq!(
            db.append_agent_chat_message("t", r#"{"type":"user_message","text":"visible"}"#)
                .unwrap(),
            Some(2)
        );
        db.append_agent_chat_message("t", journal).unwrap();
        db
    }

    #[test]
    fn history_json5_original_full_query_preserves_correlation() {
        let mut comparisons = Vec::new();
        for journal in json5_journals() {
            let db = json5_fixture(journal);
            let control = json5_fixture(journal);
            control
                .conn
                .lock()
                .unwrap()
                .execute_batch("DROP INDEX idx_agent_chat_messages_user_control;")
                .unwrap();
            let validity: (i64, i64, String) = db
                .conn
                .lock()
                .unwrap()
                .query_row(
                    "SELECT json_valid(?1),json_valid(?1,2),sqlite_version()",
                    [journal],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            assert_eq!(validity, (0, 1, "3.45.0".into()));
            let rows = json5_rows(&db, "t");
            assert_eq!(rows, json5_rows(&control, "t"));
            let original = original_last_run(&control, "t");
            assert_eq!(
                original,
                Ok(Some(
                    serde_json::json!({"type":"turn_completed","turn_id":"a","status":{"kind":"success"}})
                ))
            );
            let (candidate, sql, steps) = measured_last_run(&db, "t");
            let record = serde_json::json!({"case":"intentional_json5","journal":journal,"validity":validity,"raw_rows":rows,"original":original,"candidate":candidate,"sql":sql,"vm_steps":steps});
            println!("JSON5_PARITY {record}");
            json5_capture(record);
            comparisons.push((journal, original, candidate));
            assert_eq!(json5_rows(&db, "t"), rows);
        }
        for (journal, original, candidate) in comparisons {
            assert_eq!(candidate, original, "accepted JSON5 TEXT journal={journal}");
        }
    }

    #[tokio::test]
    async fn history_json5_native_cold_recovery_preserves_settled_status() {
        use tauri::Manager;
        let root = tempfile::tempdir().unwrap();
        let state = crate::state::AppStateStore::default();
        let workspace = state
            .create_empty_workspace_at_path(root.path().to_path_buf())
            .0;
        let db = json5_fixture(json5_journals()[0]);
        db.upsert_agent_chat_session("t", &workspace, None, "opencode")
            .unwrap();
        let original = original_last_run(&db, "t").unwrap().unwrap();
        let before = json5_rows(&db, "t");
        let db = db.control_fixture_snapshot(&root.path().join("cold.sqlite"));
        assert_eq!(json5_rows(&db, "t"), before);
        let app = tauri::test::mock_app();
        app.manage(state);
        app.manage(db);
        let observability = crate::observability::ObservabilityStore::default();
        let mut flags = observability.feature_flags();
        flags.enable_agent_chat = true;
        observability.set_feature_flags(flags);
        app.manage(observability);
        app.manage(crate::commands::agent_chat::ProviderRegistry::new());
        app.manage(crate::agent_control::NativeControlState::default());
        let status = crate::agent_control::execute(
            app.handle(),
            &crate::agent_control::ControlCaller::trusted(),
            "thread_status",
            serde_json::json!({"workspace_id":workspace,"thread_id":"t"}),
        )
        .await
        .unwrap();
        let waited = crate::agent_control::execute(app.handle(), &crate::agent_control::ControlCaller::trusted(), "thread_wait",
            serde_json::json!({"workspace_id":workspace,"thread_id":"t","turn_id":"a","timeout_ms":1})).await.unwrap();
        let record = serde_json::json!({"case":"native_cold_recovery","original":original,"raw_rows":before,"status":status,"waited":waited});
        println!("JSON5_RECOVERY {record}");
        json5_capture(record);
        assert_eq!(status["runtime_live"], false);
        assert_eq!(status["settled"],true, "canonical completion selected by original full query must recover settled status: {status}");
        assert_eq!(status["phase"], "completed");
        assert_eq!(status["turn_id"], "a");
        assert_eq!(waited["settled"], true);
        assert_eq!(waited["selected_turn"]["outcome"], "success");
    }

    fn json5_plan(db: &DatabaseStore, sql: &str, thread: &str) -> Vec<String> {
        let conn = db.conn.lock().unwrap();
        let plan = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .unwrap()
            .query_map([thread], |r| r.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(
            plan.iter().any(|line| line
                .contains("idx_agent_chat_messages_user_control (thread_id=? AND <expr>=?)")),
            "both correlation keys must be searched: {plan:?}"
        );
        plan
    }

    fn json5_parity_state(db: &DatabaseStore, thread: &str, label: &str) {
        let rows = json5_rows(db, thread);
        let original = original_last_run(db, thread);
        let (candidate, sql, steps) = measured_last_run(db, thread);
        let plan = json5_plan(db, &sql, thread);
        let record = serde_json::json!({"case":"boundary_state","label":label,"raw_rows":rows,"original":original,"candidate":candidate,"sql":sql,"vm_steps":steps,"plan":plan});
        json5_capture(record);
        assert_eq!(candidate, original, "full Value/error parity state={label}");
        assert_eq!(json5_rows(db, thread), rows);
    }

    #[test]
    fn history_json5_full_boundary_values_and_errors_preserve_original_query() {
        // Explicit durable INTEGER keys include signed extremes; appending payload
        // remains TEXT. Direct IDs only avoid AUTOINCREMENT overflow in this fixture.
        let keys = [
            i64::MIN,
            i64::MIN + 1,
            -9_007_199_254_740_993,
            -9_007_199_254_740_992,
            -1,
            0,
            1,
            2,
            9_007_199_254_740_991,
            9_007_199_254_740_992,
            9_007_199_254_740_993,
            i64::MAX - 1,
            i64::MAX,
        ];
        let base = [
            "false",
            "true",
            "0",
            "-1",
            "2",
            "2.0",
            "2.5",
            "null",
            "{}",
            "[2]",
            r#"" 2 ""#,
            r#""\t2\n""#,
            r#""2\u0000""#,
            r#""\u00002""#,
            r#""2junk""#,
            r#""2.0junk""#,
            r#""2e0junk""#,
            r#""0x2""#,
            r#""+2""#,
            r#""02""#,
            r#""2.0e0""#,
            r#""1e999""#,
            r#""-1e999""#,
            r#""not-an-id""#,
            "1e999",
            "-1e999",
            "1e-999",
            "9007199254740991",
            "9007199254740992",
            "9007199254740993",
            "9007199254740992.0",
            "9007199254740993.0",
            "9223372036854775806",
            "9223372036854775807",
            "9223372036854775808",
            "9223372036854775807.0",
            "-9223372036854775808",
            "-9223372036854775809",
            "-9223372036854775808.0",
            "+2",
            "0x2",
            "2.",
            ".2",
            "Infinity",
            "-Infinity",
            "NaN",
            "QNaN",
            "Inf",
        ];
        let mut cases = 0;
        for key in keys {
            let mut values = base.iter().map(|s| s.to_string()).collect::<Vec<_>>();
            values.extend([
                key.to_string(),
                serde_json::to_string(&key.to_string()).unwrap(),
                serde_json::to_string(&format!(" {key} ")).unwrap(),
                serde_json::to_string(&format!("{key}junk")).unwrap(),
            ]);
            values.sort();
            values.dedup();
            for token in values {
                let db = DatabaseStore::new_in_memory();
                db.upsert_agent_chat_session("t", "w", None, "opencode")
                    .unwrap();
                // Use canonical rows for every potentially selected public payload.
                {
                    let conn = db.conn.lock().unwrap();
                    conn.execute(
                        "INSERT INTO agent_chat_messages(id,thread_id,payload) VALUES(17,'t',?1)",
                        [r#"{"type":"turn_completed","turn_id":"a","status":{"kind":"success"}}"#],
                    )
                    .unwrap();
                    conn.execute(
                        "INSERT INTO agent_chat_messages(id,thread_id,payload) VALUES(?1,'t',?2)",
                        params![key, r#"{"type":"user_message","text":"boundary"}"#],
                    )
                    .unwrap();
                    let journal = format!(
                        r#"{{"type":"native_user_control","user_message_id":{token},"turn_id":"a","steered":false}}"#
                    );
                    conn.execute(
                        "INSERT INTO agent_chat_messages(id,thread_id,payload) VALUES(19,'t',?1)",
                        [journal],
                    )
                    .unwrap();
                }
                for version in [20, 18, 19] {
                    if version != 20 {
                        let conn = db.conn.lock().unwrap();
                        conn.execute_batch(&format!("DROP INDEX idx_agent_chat_messages_user_control; UPDATE schema_version SET version={version};")).unwrap();
                        super::super::create_schema(&conn).unwrap();
                    }
                    json5_parity_state(
                        &db,
                        "t",
                        &format!("key={key};token={token};from={version}"),
                    );
                    cases += 1;
                }
                // Test the actual inherited malformed SQL error, not a relaxed read.
                if key == 2 {
                    db.append_agent_chat_message("t", "malformed-json").unwrap();
                    json5_parity_state(&db, "t", &format!("malformed;token={token}"));
                    assert!(db
                        .control_last_thread_run_event("t")
                        .unwrap_err()
                        .contains("malformed JSON"));
                    cases += 1;
                }
            }
        }
        println!(
            "JSON5_BOUNDARY complete_value_error_comparisons={cases} keys={}",
            keys.len()
        );
    }

    #[test]
    fn history_json5_fresh_shipping_upgrade_and_current_index_contract() {
        for version in [20, 18, 19] {
            let db = json5_fixture(json5_journals()[0]);
            let rows = json5_rows(&db, "t");
            let conn = db.conn.lock().unwrap();
            if version != 20 {
                conn.execute_batch(&format!("DROP INDEX idx_agent_chat_messages_user_control; UPDATE schema_version SET version={version};")).unwrap();
            }
            super::super::create_schema(&conn).unwrap();
            let definition:String=conn.query_row("SELECT sql FROM sqlite_master WHERE name='idx_agent_chat_messages_user_control'",[],|r|r.get(0)).unwrap();
            assert!(definition.contains("json_valid(payload, 2)"));
            assert!(
                definition.contains("CAST(json_extract(payload, '$.user_message_id') AS NUMERIC)")
            );
            drop(conn);
            json5_parity_state(&db, "t", &format!("index-install-from={version}"));
            assert_eq!(json5_rows(&db, "t"), rows);
            let conn = db.conn.lock().unwrap();
            super::super::create_schema(&conn).unwrap();
            let reopened:String=conn.query_row("SELECT sql FROM sqlite_master WHERE name='idx_agent_chat_messages_user_control'",[],|r|r.get(0)).unwrap();
            assert_eq!(reopened, definition);
            drop(conn);
            json5_capture(
                serde_json::json!({"case":"index_contract","from_version":version,"definition":definition,"raw_rows":rows}),
            );
        }
        // The canonical-only index existed only in the unpublished first
        // candidate. IF NOT EXISTS does not rebuild it: do not reuse that DB
        // as a final runtime or claim this is a shipped schema migration.
        let db = json5_fixture(json5_journals()[0]);
        let conn = db.conn.lock().unwrap();
        conn.execute_batch("DROP INDEX idx_agent_chat_messages_user_control; CREATE INDEX idx_agent_chat_messages_user_control ON agent_chat_messages(thread_id,CAST(json_extract(payload,'$.user_message_id') AS NUMERIC)) WHERE json_valid(payload) AND json_extract(payload,'$.type')='native_user_control';").unwrap();
        let before: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name='idx_agent_chat_messages_user_control'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        super::super::create_schema(&conn).unwrap();
        let after: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name='idx_agent_chat_messages_user_control'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(after, before);
        json5_capture(
            serde_json::json!({"case":"unshipped_guard1_not_rebuilt","definition":after,"supported_final_runtime":false}),
        );
    }

    #[test]
    fn history_json5_large_correlated_and_legacy_cost_plan_and_full_output_parity() {
        let turns = 1_200;
        let budget = turns * 500; // Same 600000 VM-step contract; never increased.
        for correlated in [true, false] {
            let db = DatabaseStore::new_in_memory();
            db.upsert_agent_chat_session("t", "w", None, "opencode")
                .unwrap();
            for i in 0..turns {
                let turn = format!("run-{i}");
                let user = db
                    .append_agent_chat_message(
                        "t",
                        &serde_json::json!({"type":"user_message","text":format!("task {i}")})
                            .to_string(),
                    )
                    .unwrap()
                    .unwrap();
                if correlated {
                    // Raw JSON5 journal, deliberately bypassing canonical serde output.
                    let journal = format!(
                        r#"{{type:'native_user_control',user_message_id:{user},turn_id:'{turn}',steered:false,}}"#
                    );
                    db.append_agent_chat_message("t", &journal).unwrap();
                }
                for part in 0..3 {
                    let payload=serde_json::json!({"type":"item_completed","turn_id":turn,"item":{"kind":"assistant_text","text":format!("response {i} part {part}")}}).to_string();
                    // Legacy corpus retains exactly 6000 rows, with nonselected
                    // JSON5 assistant TEXT, not an invented legacy correlation.
                    let payload = if correlated {
                        payload
                    } else {
                        format!("{},}}", payload.strip_suffix('}').unwrap())
                    };
                    db.append_agent_chat_message("t", &payload).unwrap();
                }
                db.append_agent_chat_message("t",&serde_json::json!({"type":"turn_completed","turn_id":turn,"status":{"kind":"success"}}).to_string()).unwrap();
            }
            let rows = json5_rows(&db, "t");
            assert_eq!(rows.len(), if correlated { 7200 } else { 6000 });
            let (candidate, sql, steps) = measured_last_run(&db, "t");
            let plan = json5_plan(&db, &sql, "t");
            db.conn
                .lock()
                .unwrap()
                .execute_batch("DROP INDEX idx_agent_chat_messages_user_control;")
                .unwrap();
            let original = original_last_run(&db, "t");
            assert_eq!(candidate, original);
            assert_eq!(json5_rows(&db, "t"), rows);
            let record = serde_json::json!({"case":"json5_cost_corpus","correlated":correlated,"turns":turns,"rows":rows,"row_count":rows.len(),"vm_steps":steps,"budget":budget,"plan":plan,"original":original,"candidate":candidate});
            println!("JSON5_COST correlated={correlated} turns={turns} rows={} vm_steps={steps} budget={budget} plan={plan:?} original={original:?} candidate={candidate:?}",rows.len());
            json5_capture(record);
            assert!(
                steps > 0 && steps <= budget,
                "unchanged JSON5 history budget: {steps}>{budget}"
            );
        }
    }

    #[test]
    fn history_json5_text_domain_admission_and_parser_errors_remain_unchanged() {
        let payloads = [
            "null",
            "false",
            "2",
            "1e999",
            "-1e999",
            "NaN",
            "Infinity",
            "{}",
            "[]",
            "malformed-json",
            r#"{"type":"turn_completed","turn_id":"a","status":{"kind":"success"},}"#,
            r#"{"type":"user_message","text":"json5",}"#,
            r#"{"type":"native_user_control","user_message_id":2,"turn_id":"a","steered":false,}"#,
            "{\"type\":\"native_user_control\",\"user_message_id\":2}\0junk",
        ];
        for payload in payloads {
            let db = DatabaseStore::new_in_memory();
            let control = DatabaseStore::new_in_memory();
            control
                .conn
                .lock()
                .unwrap()
                .execute_batch("DROP INDEX idx_agent_chat_messages_user_control;")
                .unwrap();
            for fixture in [&db, &control] {
                fixture
                    .upsert_agent_chat_session("t", "w", None, "opencode")
                    .unwrap();
                assert!(fixture
                    .append_agent_chat_message("t", payload)
                    .unwrap()
                    .is_some());
            }
            let rows = json5_rows(&db, "t");
            assert_eq!(rows, json5_rows(&control, "t"));
            assert_eq!(rows[0].2, "text");
            let original = original_last_run(&control, "t");
            let candidate = db.control_last_thread_run_event("t");
            json5_capture(
                serde_json::json!({"case":"text_domain","raw_rows":rows,"original":original,"candidate":candidate}),
            );
            assert_eq!(
                candidate, original,
                "accepted raw TEXT/parser errors payload={payload:?}"
            );
        }
        // Native locked API probe only, not an inserted BLOB or a supported append
        // path. jsonb inputs are outside append_agent_chat_message(&str)'s contract.
        let db = DatabaseStore::new_in_memory();
        let domain:(String,i64,i64)=db.conn.lock().unwrap().query_row(
            "SELECT typeof(jsonb('{\"id\":2}')),json_valid(jsonb('{\"id\":2}'),2),json_extract(jsonb('{\"id\":2}'),'$.id')",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(domain, ("blob".into(), 0, 2));
        json5_capture(
            serde_json::json!({"case":"blob_outside_text_append_contract","native_probe":domain,"blob_inserted":false}),
        );
    }

    #[test]
    fn history_json5_logical_states_preserve_full_values_errors_and_raw_rows() {
        let db = DatabaseStore::new_in_memory();
        db.upsert_agent_chat_session("t", "w", None, "opencode")
            .unwrap();
        json5_parity_state(&db, "t", "empty");
        db.append_agent_chat_message("t", r#"{"type":"user_message","text":"immediate"}"#)
            .unwrap();
        json5_parity_state(&db, "t", "legacy-user");
        // Without a canonical completion, the selected JSON5 journal must
        // retain the original serde_json decoding error, not be normalized.
        db.append_agent_chat_message(
            "t",
            r#"{type:'native_user_control',user_message_id:1,turn_id:'a',steered:false,}"#,
        )
        .unwrap();
        json5_parity_state(&db, "t", "json5-selected-parser-error");
        assert!(db.control_last_thread_run_event("t").is_err());
        db.append_agent_chat_message(
            "t",
            r#"{"type":"turn_completed","turn_id":"a","status":{"kind":"success"}}"#,
        )
        .unwrap();
        json5_parity_state(&db, "t", "a-completed");
        db.append_agent_chat_message(
            "t",
            r#"{"type":"turn_completed","turn_id":"b","status":{"kind":"success"}}"#,
        )
        .unwrap();
        json5_parity_state(&db, "t", "b-completed");
        db.control_append_user_message(
            "t",
            r#"{"type":"user_message","text":"newer c"}"#,
            "c",
            false,
        )
        .unwrap();
        json5_parity_state(&db, "t", "newer-c");
        let delayed = db
            .append_agent_chat_message("t", r#"{"type":"user_message","text":"late b"}"#)
            .unwrap()
            .unwrap();
        db.append_agent_chat_message("t", &format!(r#"{{type:'native_user_control',user_message_id:{delayed},turn_id:'b',steered:false,}}"#)).unwrap();
        json5_parity_state(&db, "t", "late-json5-b-correlation");
        db.control_append_user_message(
            "t",
            r#"{"type":"user_message","text":"guidance","steered_turn_id":"c"}"#,
            "c",
            true,
        )
        .unwrap();
        json5_parity_state(&db, "t", "steered-guidance");
        db.append_agent_chat_message(
            "t",
            r#"{"type":"turn_completed","turn_id":"c","status":{"kind":"interrupted"}}"#,
        )
        .unwrap();
        json5_parity_state(&db, "t", "c-interrupted");
        db.append_agent_chat_message("t", r#"{"type":"user_message","text":"unknown legacy"}"#)
            .unwrap();
        json5_parity_state(&db, "t", "newer-legacy");
        db.upsert_agent_chat_session("foreign", "w", None, "opencode")
            .unwrap();
        json5_parity_state(&db, "foreign", "foreign-empty");
        db.append_agent_chat_message("t", "malformed-json").unwrap();
        json5_parity_state(&db, "t", "malformed-read-error");
        json5_parity_state(&db, "foreign", "foreign-after-malformed");
    }

    // Measure the statement executed by the real owner, not a simplified query.
    fn measured_last_run(
        db: &DatabaseStore,
        thread: &str,
    ) -> (Result<Option<Value>, String>, String, i32) {
        use std::ffi::{c_void, CStr};
        #[derive(Default)]
        struct Trace {
            sql: String,
            steps: i32,
        }
        unsafe extern "C" fn profile(
            kind: u32,
            context: *mut c_void,
            stmt: *mut c_void,
            _: *mut c_void,
        ) -> i32 {
            if kind != rusqlite::ffi::SQLITE_TRACE_PROFILE as u32 {
                return 0;
            }
            let stmt = stmt as *mut rusqlite::ffi::sqlite3_stmt;
            let ptr = unsafe { rusqlite::ffi::sqlite3_sql(stmt) };
            if ptr.is_null() {
                return 0;
            }
            let sql = unsafe { CStr::from_ptr(ptr) }.to_string_lossy();
            if sql.starts_with("WITH evidence AS") {
                let trace = unsafe { &mut *(context as *mut Trace) };
                trace.sql = sql.into_owned();
                trace.steps = unsafe {
                    rusqlite::ffi::sqlite3_stmt_status(
                        stmt,
                        rusqlite::ffi::SQLITE_STMTSTATUS_VM_STEP,
                        0,
                    )
                };
            }
            0
        }
        let mut trace = Box::<Trace>::default();
        {
            let conn = db.conn.lock().unwrap();
            unsafe {
                assert_eq!(
                    rusqlite::ffi::sqlite3_trace_v2(
                        conn.handle(),
                        rusqlite::ffi::SQLITE_TRACE_PROFILE as u32,
                        Some(profile),
                        &mut *trace as *mut Trace as *mut c_void
                    ),
                    0
                );
            }
        }
        let result = db.control_last_thread_run_event(thread);
        {
            let conn = db.conn.lock().unwrap();
            unsafe {
                rusqlite::ffi::sqlite3_trace_v2(conn.handle(), 0, None, std::ptr::null_mut());
            }
        }
        (result, trace.sql, trace.steps)
    }

    #[test]
    fn history_correlation_cost_is_bounded_for_correlated_and_legacy_histories() {
        let turns = 1_200;
        let budget = turns * 500;
        let mut costs = Vec::new();
        for correlated in [true, false] {
            let db = DatabaseStore::new_in_memory();
            db.upsert_agent_chat_session("cost", "w", None, "opencode")
                .unwrap();
            for i in 0..turns {
                let turn = format!("run-{i}");
                let user = serde_json::json!({"type":"user_message","thread_id":"cost","text":format!("task {i}")}).to_string();
                if correlated {
                    db.control_append_user_message("cost", &user, &turn, false)
                        .unwrap()
                        .unwrap();
                } else {
                    db.append_agent_chat_message("cost", &user)
                        .unwrap()
                        .unwrap();
                }
                for part in 0..3 {
                    db.append_agent_chat_message("cost", &serde_json::json!({"type":"item_completed","thread_id":"cost",
                        "turn_id":turn,"item":{"kind":"assistant_text","text":format!("response {i} part {part}")}}).to_string()).unwrap();
                }
                db.append_agent_chat_message(
                    "cost",
                    &serde_json::json!({"type":"turn_completed","thread_id":"cost",
                    "turn_id":turn,"status":{"kind":"success"}})
                    .to_string(),
                )
                .unwrap();
            }
            let (result, sql, steps) = measured_last_run(&db, "cost");
            let expected = serde_json::json!({"type":"turn_completed","thread_id":"cost","turn_id":format!("run-{}", turns-1),"status":{"kind":"success"}});
            assert_eq!(result.unwrap(), Some(expected.clone()));
            let conn = db.conn.lock().unwrap();
            let plan = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap()
                .query_map(["cost"], |r| r.get::<_, String>(3))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            let rows: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM agent_chat_messages WHERE thread_id='cost'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            drop(conn);
            let mut timings = Vec::new();
            for _ in 0..3 {
                let start = std::time::Instant::now();
                assert_eq!(
                    db.control_last_thread_run_event("cost").unwrap(),
                    Some(expected.clone())
                );
                timings.push(start.elapsed().as_micros());
            }
            println!("HISTORY_COST correlated={correlated} turns={turns} rows={rows} vm_steps={steps} budget={budget} native_us={timings:?} plan={plan:?} result={expected}");
            costs.push((correlated, steps));
        }
        for (correlated, steps) in costs {
            assert!(steps > 0 && steps <= budget, "history correlation exceeds unchanged VM-step budget: correlated={correlated}, steps={steps}, budget={budget}");
        }
    }

    #[test]
    fn history_correlation_preserves_numeric_affinity_and_payload_admission() {
        // INTEGER row-id affinity also admits numeric JSON strings and reals.
        for value in [
            serde_json::json!(2),
            serde_json::json!(2.0),
            serde_json::json!("2"),
            serde_json::json!("02"),
            serde_json::json!("2.0e0"),
            serde_json::json!(true),
            serde_json::json!(null),
            serde_json::json!("not-an-id"),
            serde_json::json!({"id":2}),
            serde_json::json!([2]),
            serde_json::json!(2.5),
        ] {
            let db = DatabaseStore::new_in_memory();
            db.upsert_agent_chat_session("t", "w", None, "opencode")
                .unwrap();
            db.append_agent_chat_message(
                "t",
                r#"{"type":"turn_completed","turn_id":"a","status":{"kind":"success"}}"#,
            )
            .unwrap();
            let row = db
                .append_agent_chat_message("t", r#"{"type":"user_message","text":"visible"}"#)
                .unwrap()
                .unwrap();
            assert_eq!(row, 2);
            db.append_agent_chat_message("t", &serde_json::json!({"type":"native_user_control","user_message_id":value,"turn_id":"a","steered":false}).to_string()).unwrap();
            let expected_correlated: bool = db.conn.lock().unwrap().query_row(
                "SELECT EXISTS(SELECT 1 FROM agent_chat_messages a JOIN agent_chat_messages b ON b.thread_id=a.thread_id
                    WHERE a.id=2 AND json_extract(b.payload,'$.type')='native_user_control' AND json_extract(b.payload,'$.user_message_id')=a.id)", [], |r|r.get(0)).unwrap();
            let event = db.control_last_thread_run_event("t").unwrap().unwrap();
            assert_eq!(
                event["type"],
                if expected_correlated {
                    "turn_completed"
                } else {
                    "user_message"
                },
                "numeric-affinity value={value}"
            );
            // Invalid JSON was already admitted by the existing guarded FTS trigger.
            assert!(db
                .append_agent_chat_message("t", "malformed-json")
                .unwrap()
                .is_some());
            assert!(db
                .control_last_thread_run_event("t")
                .unwrap_err()
                .contains("malformed JSON"));
            let foreign = db.upsert_agent_chat_session("foreign", "w", None, "opencode");
            foreign.unwrap();
            assert!(db
                .control_last_thread_run_event("foreign")
                .unwrap()
                .is_none());
        }
    }

    #[test]
    fn history_full_query_parity_and_index_upgrade_preserve_atomic_envelopes() {
        let db = DatabaseStore::new_in_memory();
        db.upsert_agent_chat_session("t", "w", None, "opencode")
            .unwrap();
        let parity = || {
            let reference: Option<String> = db
                .conn
                .lock()
                .unwrap()
                .query_row(ORIGINAL_QUERY, ["t"], |r| r.get(0))
                .optional()
                .unwrap();
            let expected = reference.map(|s| serde_json::from_str::<Value>(&s).unwrap());
            assert_eq!(db.control_last_thread_run_event("t").unwrap(), expected);
            println!("HISTORY_PARITY result={expected:?}");
        };
        parity();
        let first = db
            .control_append_user_message(
                "t",
                r#"{"type":"user_message","text":"immediate"}"#,
                "a",
                false,
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            first, 1,
            "cursor identifies visible envelope, not hidden metadata"
        );
        assert_eq!(
            db.list_agent_chat_messages_after("t", Some(first))
                .unwrap()
                .len(),
            1
        );
        parity();
        db.append_agent_chat_message(
            "t",
            r#"{"type":"turn_completed","turn_id":"a","status":{"kind":"success"}}"#,
        )
        .unwrap();
        parity();
        db.append_agent_chat_message(
            "t",
            r#"{"type":"turn_completed","turn_id":"b","status":{"kind":"success"}}"#,
        )
        .unwrap();
        parity();
        db.control_append_user_message(
            "t",
            r#"{"type":"user_message","text":"newer c"}"#,
            "c",
            false,
        )
        .unwrap();
        parity();
        db.control_append_user_message(
            "t",
            r#"{"type":"user_message","text":"deferred b"}"#,
            "b",
            false,
        )
        .unwrap();
        parity();
        db.control_append_user_message(
            "t",
            r#"{"type":"user_message","text":"steering","steered_turn_id":"c"}"#,
            "c",
            true,
        )
        .unwrap();
        parity();
        db.append_agent_chat_message(
            "t",
            r#"{"type":"turn_completed","turn_id":"c","status":{"kind":"interrupted"}}"#,
        )
        .unwrap();
        parity();
        db.control_append_user_message(
            "t",
            r#"{"type":"user_message","text":"empty id legacy"}"#,
            "",
            false,
        )
        .unwrap();
        parity();
        let raw = db.list_agent_chat_messages("t");
        let page = db
            .read_agent_chat_history_page("w", "t", None, 100)
            .unwrap();
        assert_eq!(page.total_visible_messages, 5);
        assert!(!serde_json::to_string(&page)
            .unwrap()
            .contains("native_user_control"));
        {
            let conn = db.conn.lock().unwrap();
            conn.execute_batch("DROP INDEX idx_agent_chat_messages_user_control; UPDATE schema_version SET version=18;").unwrap();
            for _ in 0..2 {
                super::super::create_schema(&conn).unwrap();
            }
            let present: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='idx_agent_chat_messages_user_control')", [], |r|r.get(0)).unwrap();
            assert!(present);
        }
        assert_eq!(db.list_agent_chat_messages("t"), raw);
        assert_eq!(
            db.read_agent_chat_history_page("w", "t", None, 100)
                .unwrap(),
            page
        );
        parity();
        db.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM agent_chat_sessions WHERE thread_id='t'", [])
            .unwrap();
        assert!(db.control_last_thread_run_event("t").unwrap().is_none());
        assert!(db
            .control_append_user_message(
                "t",
                r#"{"type":"user_message","text":"deleted"}"#,
                "d",
                false
            )
            .unwrap()
            .is_none());
        assert!(db.list_agent_chat_messages("t").is_empty());
        // Guarded index creation must not change old malformed-row admission.
        db.upsert_agent_chat_session("t", "w", None, "opencode")
            .unwrap();
        assert!(db
            .append_agent_chat_message("t", "malformed-json")
            .unwrap()
            .is_some());
        {
            let conn = db.conn.lock().unwrap();
            conn.execute_batch("DROP INDEX idx_agent_chat_messages_user_control;")
                .unwrap();
            super::super::create_schema(&conn).unwrap();
            let original_error = conn
                .query_row(ORIGINAL_QUERY, ["t"], |r| r.get::<_, String>(0))
                .unwrap_err()
                .to_string();
            drop(conn);
            assert_eq!(
                db.control_last_thread_run_event("t").unwrap_err(),
                original_error
            );
        }
        // The existing async-question DELETE trigger also rejects invalid JSON.
        assert!(db
            .conn
            .lock()
            .unwrap()
            .execute("DELETE FROM agent_chat_sessions WHERE thread_id='t'", [])
            .unwrap_err()
            .to_string()
            .contains("malformed JSON"));
    }

    #[test]
    fn mario_r3_real_queue_dispositions_win_even_before_late_acceptance_ack() {
        use crate::agent_provider::{ProviderRuntimeEvent, ThreadId, TurnId};
        for dispatched in [false,true] {
            let db = DatabaseStore::new_in_memory();
            db.upsert_agent_chat_session("queue-thread","workspace",None,"cursor").unwrap();
            let event = if dispatched {
                ProviderRuntimeEvent::QueuedTurnDispatched {
                    thread_id:ThreadId("queue-thread".into()),queued_id:"opaque-queue".into(),turn_id:TurnId("b".into()),
                    text:"synthetic private prompt".into(),steered:false,
                }
            } else {
                ProviderRuntimeEvent::QueuedTurnCancelled {thread_id:ThreadId("queue-thread".into()),queued_id:"opaque-queue".into()}
            };
            db.control_record_queue_event(&event).unwrap();
            db.control_record_queue_admission("queue-thread","opaque-queue").unwrap();
            db.control_record_queue_admission("queue-thread","opaque-queue").unwrap();
            assert!(db.control_unresolved_queued_ids("queue-thread").unwrap().is_empty());
            let rows = db.list_agent_chat_messages("queue-thread");
            assert_eq!(rows.len(),2, "late ACK and event bridge must not duplicate admission");
            assert!(!rows.iter().any(|row|row.contains("private prompt")));
            if dispatched {
                assert_eq!(db.control_last_thread_run_event("queue-thread").unwrap().unwrap()["disposition"],"dispatched",
                    "dispatch without a later completion is not an old parent's terminal outcome");
            } else {
                assert!(db.control_last_thread_run_event("queue-thread").unwrap().is_none(), "cancelled queue is not a new parent run");
            }
        }
    }

    #[test]
    fn review5479417968_late_envelopes_do_not_reorder_logical_runs_or_guess_legacy_ids() {
        use crate::agent_provider::{ProviderRuntimeEvent, ThreadId, TurnId};
        let db = DatabaseStore::new_in_memory();
        db.upsert_agent_chat_session("t","w",None,"opencode").unwrap();
        let complete = |id: &str| serde_json::json!({"type":"turn_completed","thread_id":"t","turn_id":id,"status":{"kind":"success"}}).to_string();
        db.append_agent_chat_message("t",&complete("a")).unwrap();
        db.append_agent_chat_message("t",&complete("b")).unwrap();
        db.control_append_user_message("t",r#"{"type":"user_message","thread_id":"t","text":"visible c"}"#,"c",false).unwrap().unwrap();
        // B's delayed ACK and actual deferred envelope arrive after C starts.
        db.control_record_queue_event(&ProviderRuntimeEvent::QueuedTurnDispatched {thread_id:ThreadId("t".into()),queued_id:"qb".into(),turn_id:TurnId("b".into()),text:"private b".into(),steered:false}).unwrap();
        db.control_append_user_message("t",r#"{"type":"user_message","thread_id":"t","text":"visible b"}"#,"b",false).unwrap().unwrap();
        assert_eq!(db.control_last_thread_run_event("t").unwrap().unwrap()["turn_id"],"c");
        assert!(db.control_turn_outcome("t","c").unwrap().is_none());
        assert!(db.control_turn_outcome("t","b").unwrap().is_some());
        db.append_agent_chat_message("t",&complete("c")).unwrap();
        assert_eq!(db.control_last_thread_run_event("t").unwrap().unwrap()["type"],"turn_completed");
        db.append_agent_chat_message("t",r#"{"type":"user_message","thread_id":"t","text":"unknown legacy"}"#).unwrap();
        let legacy = db.control_last_thread_run_event("t").unwrap().unwrap();
        assert_eq!(legacy["type"],"user_message");assert!(legacy["turn_id"].is_null());
        db.append_agent_chat_message("t",r#"{"type":"user_message","thread_id":"t","text":"late steer","steered_turn_id":"b"}"#).unwrap();
        assert_eq!(db.control_last_thread_run_event("t").unwrap().unwrap(),legacy,"guidance is not a new run");
        assert!(db.control_append_user_message("missing",r#"{"type":"user_message","text":"deleted thread"}"#,"invented",false).unwrap().is_none());
        assert!(db.control_last_thread_run_event("missing").unwrap().is_none(),"deleted thread must not acquire a journal identity");
        let page = db.read_agent_chat_history_page("w","t",None,100).unwrap();
        assert_eq!(page.total_visible_messages,4);
        assert!(!serde_json::to_string(&page).unwrap().contains("native_user_control"));
    }

    #[test]
    fn retry_of_admitted_operation_returns_same_receipt() {
        let db = DatabaseStore::new_in_memory();
        let first = db.admit_control_operation("client-a","req-a","thread_launch","hash-a","boot-a",Some("ws-a"),Some("thread-a")).unwrap();
        let retry = db.admit_control_operation("client-a","req-a","thread_launch","hash-a","boot-a",Some("ws-a"),Some("thread-a")).unwrap();
        assert!(first.newly_admitted);
        assert!(!retry.newly_admitted, "retry must not admit a second launch");
        assert_eq!(first.receipt.operation_id, retry.receipt.operation_id);
    }

    #[test]
    fn prior_process_dispatch_is_uncertain_not_retried() {
        let db = DatabaseStore::new_in_memory();
        let first = db.admit_control_operation("client-a","req-a","thread_launch","hash-a","boot-a",Some("ws-a"),Some("thread-a")).unwrap();
        let retry = db.admit_control_operation("client-a","req-a","thread_launch","hash-a","boot-b",Some("ws-a"),Some("thread-a")).unwrap();
        assert!(!retry.newly_admitted);
        assert_eq!(first.receipt.operation_id, retry.receipt.operation_id);
        assert_eq!(retry.receipt.state, "needs_attention", "a restart must not turn uncertain dispatch into a duplicate run");
    }
}
