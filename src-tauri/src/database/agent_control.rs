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
             AND json_extract(payload,'$.type') IN ('turn_completed','user_message','turn_queued','queued_turn_dispatched')
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
