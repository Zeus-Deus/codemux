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

    pub fn control_last_thread_run_event(&self, thread: &str) -> Result<Option<Value>, String> {
        let event: Option<String> = self.conn.lock().unwrap().query_row(
            "SELECT payload FROM agent_chat_messages WHERE thread_id=?1
             AND (json_extract(payload,'$.type') IN ('turn_completed','user_message','turn_queued','queued_turn_dispatched')
                  OR (json_extract(payload,'$.type')='native_queue_control'
                      AND json_extract(payload,'$.disposition')='dispatched' AND json_extract(payload,'$.steered')=0))
             ORDER BY id DESC LIMIT 1",
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
