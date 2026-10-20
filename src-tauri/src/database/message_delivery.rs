//! Message metadata and the durable delivery/outbox journal.
use super::DatabaseStore;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(super) fn migrate(conn: &Connection) -> Result<(), String> {
    for (table, column, definition) in [
        ("agent_chat_messages", "message_id", "TEXT"),
        (
            "agent_chat_messages",
            "created_by",
            "TEXT NOT NULL DEFAULT 'user'",
        ),
        (
            "agent_chat_messages",
            "creation_source",
            "TEXT NOT NULL DEFAULT 'unknown'",
        ),
        ("agent_chat_messages", "sender_thread_id", "TEXT"),
        ("agent_chat_messages", "sender_title", "TEXT"),
        (
            "agent_chat_messages",
            "input_intent",
            "TEXT NOT NULL DEFAULT 'turn_start'",
        ),
        ("agent_chat_messages", "notification", "TEXT"),
        (
            "agent_chat_queued_turns",
            "provenance",
            "TEXT NOT NULL DEFAULT '{}'",
        ),
        ("agent_control_operations", "admitted_payload", "TEXT"),
        ("agent_control_operations", "dispatch_disposition", "TEXT"),
        ("agent_control_operations", "delivery_id", "TEXT"),
    ] {
        let present = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|e| e.to_string())?
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|e| e.to_string())?
            .any(|name| name.is_ok_and(|name| name == column));
        if !present {
            conn.execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN {column} {definition}"
            ))
            .map_err(|e| e.to_string())?;
        }
    }
    conn.execute_batch("
        CREATE UNIQUE INDEX IF NOT EXISTS agent_chat_message_identity
            ON agent_chat_messages(thread_id,message_id) WHERE message_id IS NOT NULL;
        CREATE TRIGGER IF NOT EXISTS agent_chat_message_provenance_insert
        AFTER INSERT ON agent_chat_messages
        WHEN json_valid(new.payload,2) AND json_extract(new.payload,'$.type')='user_message'
        BEGIN
            UPDATE agent_chat_messages SET message_id=json_extract(new.payload,'$.message_id'),
                created_by=COALESCE(json_extract(new.payload,'$.created_by'),'user'),
                creation_source=COALESCE(json_extract(new.payload,'$.creation_source'),'unknown'),
                sender_thread_id=json_extract(new.payload,'$.sender_thread_id'),sender_title=json_extract(new.payload,'$.sender_title'),
                input_intent=COALESCE(json_extract(new.payload,'$.input_intent'),'turn_start'),notification=json_extract(new.payload,'$.notification')
            WHERE id=new.id;
        END;
        CREATE TABLE IF NOT EXISTS agent_message_deliveries (
            id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, message_id TEXT NOT NULL,
            payload TEXT NOT NULL, dispatch_payload TEXT, epoch TEXT NOT NULL, generation INTEGER NOT NULL,
            attempt_id TEXT NOT NULL, state TEXT NOT NULL DEFAULT 'pending',
            principal TEXT, operation_id TEXT, queued_id TEXT, turn_id TEXT,
            result TEXT, error TEXT, attempted INTEGER NOT NULL DEFAULT 0,
            retry_at TEXT, created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(thread_id,message_id)
        );
        CREATE TABLE IF NOT EXISTS agent_message_delivery_results (
            delivery_id TEXT NOT NULL REFERENCES agent_message_deliveries(id) ON DELETE CASCADE,
            task_id TEXT NOT NULL, result_version INTEGER NOT NULL DEFAULT 1,
            target_thread_id TEXT NOT NULL,
            PRIMARY KEY(delivery_id,task_id,result_version),
            UNIQUE(task_id,result_version,target_thread_id)
        );
        CREATE TABLE IF NOT EXISTS agent_thread_delivery_state (
            thread_id TEXT PRIMARY KEY, generation INTEGER NOT NULL DEFAULT 0, stopped INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS agent_message_delivery_pending ON agent_message_deliveries(state,retry_at);
    ").map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryRecord {
    pub id: String,
    pub thread_id: String,
    pub message_id: String,
    pub payload: Value,
    pub dispatch_payload: Option<Value>,
    pub epoch: String,
    pub generation: u64,
    pub attempt_id: String,
    pub state: String,
    pub principal: Option<String>,
    pub operation_id: Option<String>,
    pub queued_id: Option<String>,
    pub turn_id: Option<String>,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub attempted: bool,
}
fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<DeliveryRecord> {
    let payload: String = row.get(3)?;
    let result: Option<String> = row.get(12)?;
    Ok(DeliveryRecord {
        id: row.get(0)?,
        thread_id: row.get(1)?,
        message_id: row.get(2)?,
        payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
        dispatch_payload: row
            .get::<_, Option<String>>(15)?
            .and_then(|v| serde_json::from_str(&v).ok()),
        epoch: row.get(4)?,
        generation: row.get(5)?,
        attempt_id: row.get(6)?,
        state: row.get(7)?,
        principal: row.get(8)?,
        operation_id: row.get(9)?,
        queued_id: row.get(10)?,
        turn_id: row.get(11)?,
        result: result.and_then(|v| serde_json::from_str(&v).ok()),
        error: row.get(13)?,
        attempted: row.get(14)?,
    })
}
const COLUMNS: &str = "id,thread_id,message_id,payload,epoch,generation,attempt_id,state,principal,operation_id,queued_id,turn_id,result,error,attempted,dispatch_payload";

/// Optional outside-controller receipt, admitted atomically with its effect.
pub struct DeliveryOperation<'a> {
    pub principal: &'a str,
    pub key: &'a str,
    pub hash: &'a str,
    pub workspace: &'a str,
}
impl DatabaseStore {
    pub fn thread_message_delivery_attention(
        &self,
        thread: &str,
    ) -> Result<Vec<DeliveryRecord>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt=conn.prepare(&format!("SELECT {COLUMNS} FROM agent_message_deliveries WHERE thread_id=?1 AND state IN ('held','held_unknown') ORDER BY created_at DESC LIMIT 20")).map_err(|e|e.to_string())?;
        let rows = stmt.query_map([thread], read).map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }
    pub fn rearm_saved_message_delivery(&self, id: &str, thread: &str) -> Result<(), String> {
        let (generation, _) = self.delivery_generation(thread)?;
        let n=self.conn.lock().unwrap().execute("UPDATE agent_message_deliveries SET state='preparing',generation=?3,error=NULL WHERE id=?1 AND thread_id=?2 AND attempted=0 AND state IN ('queued','held','pending')",params![id,thread,generation]).map_err(|e|e.to_string())?;
        if n == 0 {
            return Err(
                "Saved message may already have reached the provider; automatic replay is held"
                    .into(),
            );
        }
        Ok(())
    }
    pub fn delivery_generation(&self, thread: &str) -> Result<(u64, bool), String> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT generation,stopped FROM agent_thread_delivery_state WHERE thread_id=?1",
                [thread],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map(|r| r.unwrap_or((0, false)))
            .map_err(|e| e.to_string())
    }
    pub fn verify_delivery_generation(&self, thread: &str, generation: u64) -> Result<(), String> {
        let (current, stopped) = self.delivery_generation(thread)?;
        if current != generation || stopped {
            Err("delivery_held: thread was stopped after admission".into())
        } else {
            Ok(())
        }
    }
    pub fn resume_message_delivery_thread(&self, thread: &str) -> Result<(), String> {
        self.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE agent_thread_delivery_state SET stopped=0 WHERE thread_id=?1",
                [thread],
            )
            .map(drop)
            .map_err(|e| e.to_string())
    }
    pub fn stop_message_deliveries(&self, thread: &str) -> Result<(), String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute("INSERT INTO agent_thread_delivery_state (thread_id,generation,stopped) VALUES (?1,1,1) ON CONFLICT(thread_id) DO UPDATE SET generation=generation+1,stopped=1",[thread]).map_err(|e|e.to_string())?;
        tx.execute("UPDATE agent_message_deliveries SET state=CASE WHEN attempted=1 THEN 'held_unknown' ELSE 'held' END,error='Thread stopped; automatic delivery held' WHERE thread_id=?1 AND state IN ('pending','preparing','queued','submitting')",[thread]).map_err(|e|e.to_string())?;
        tx.execute("UPDATE agent_control_operations SET state='needs_attention',dispatch_disposition='held',error=json_object('code','delivery_held','message','Thread stopped') WHERE delivery_id IN (SELECT id FROM agent_message_deliveries WHERE thread_id=?1 AND state IN ('held','held_unknown'))",[thread]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }

    pub fn admit_message_delivery(
        &self,
        id: &str,
        thread: &str,
        message: &str,
        payload: &Value,
        epoch: &str,
        generation: u64,
        operation: Option<DeliveryOperation<'_>>,
        claim_immediately: bool,
    ) -> Result<(String, bool), String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        if let Some(op) = &operation {
            let prior: Option<(String,String,String)> = tx.query_row(
                "SELECT id,tool,payload_hash FROM agent_control_operations WHERE principal=?1 AND request_key=?2",
                params![op.principal,op.key], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|e|e.to_string())?;
            if let Some((id, tool, hash)) = prior {
                if tool != "thread_send" || hash != op.hash {
                    return Err("request_key_conflict: retry arguments must be unchanged".into());
                }
                return Ok((id, false));
            }
        }
        let prior: Option<(String,String)> = tx.query_row(
            "SELECT id,payload FROM agent_message_deliveries WHERE thread_id=?1 AND message_id=?2",
            params![thread,message], |r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|e.to_string())?;
        if let Some((id, prior)) = prior {
            if serde_json::from_str::<Value>(&prior).ok().as_ref() != Some(payload) {
                return Err("message_identity_conflict: message payload changed".into());
            }
            return Ok((id, false));
        }
        let (principal, operation_id) = match &operation {
            Some(op) => {
                tx.execute("INSERT INTO agent_control_operations (id,principal,request_key,tool,payload_hash,epoch,state,workspace_id,thread_id,admitted_payload,dispatch_disposition,delivery_id)
                    VALUES (?1,?2,?3,'thread_send',?4,?5,'accepted',?6,?7,?8,'pending',?1)",
                    params![id,op.principal,op.key,op.hash,epoch,op.workspace,thread,payload.to_string()]).map_err(|e|e.to_string())?;
                (Some(op.principal), Some(id))
            }
            None => (None, None),
        };
        tx.execute("INSERT INTO agent_message_deliveries (id,thread_id,message_id,payload,epoch,generation,attempt_id,principal,operation_id,state)
            VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![id,thread,message,payload.to_string(),epoch,generation,id,principal,operation_id,if claim_immediately {"preparing"}else{"pending"}]).map_err(|e|e.to_string())?;
        let source = &payload["provenance"]["notification"]["source"];
        let terminal = payload["provenance"]["notification"]["outcome"]
            .as_str()
            .is_some_and(|outcome| outcome != "updated");
        if terminal
            && source["kind"]
                .as_str()
                .is_some_and(|kind| matches!(kind, "delegated_task" | "remote_task" | "subagent"))
        {
            // Existing terminal task reports have one immutable version. PR2's
            // task publisher can supply later canonical result versions here.
            let version = payload["provenance"]["notification"]["result_version"]
                .as_u64()
                .unwrap_or(1);
            for task in source["task_ids"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                tx.execute("INSERT INTO agent_message_delivery_results (delivery_id,task_id,result_version,target_thread_id) VALUES (?1,?2,?3,?4)",params![id,task,version,thread])
                    .map_err(|e|format!("result_already_admitted: terminal result delivery is already owned: {e}"))?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok((id.into(), true))
    }
    pub fn message_delivery(&self, id: &str) -> Result<Option<DeliveryRecord>, String> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                &format!("SELECT {COLUMNS} FROM agent_message_deliveries WHERE id=?1"),
                [id],
                read,
            )
            .optional()
            .map_err(|e| e.to_string())
    }
    pub fn pending_message_deliveries(&self) -> Result<Vec<DeliveryRecord>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM agent_message_deliveries WHERE state='pending' AND (json_extract(payload,'$.origin') IN ('controller','pr_watch') OR (json_extract(payload,'$.origin')='delegation' AND json_extract(payload,'$.provenance.notification.source.kind')='delegated_task')) AND (retry_at IS NULL OR retry_at<=datetime('now')) ORDER BY created_at LIMIT 32")).map_err(|e|e.to_string())?;
        let rows = stmt.query_map([], read).map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }
    pub fn claim_message_delivery(&self, id: &str) -> Result<bool, String> {
        self.conn.lock().unwrap().execute("UPDATE agent_message_deliveries SET state='preparing',updated_at=datetime('now') WHERE id=?1 AND state='pending'",[id]).map(|n|n==1).map_err(|e|e.to_string())
    }
    /// Called at the adapter's actual dispatch boundary, after all fallible
    /// authority checks. A crash from this point is possible acceptance.
    pub fn message_delivery_submitting(&self, id: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute("UPDATE agent_message_deliveries SET state=CASE WHEN state='accepted' THEN state ELSE 'submitting' END,attempted=1,updated_at=datetime('now') WHERE id=?1 AND state IN ('preparing','queued','submitting','accepted')",[id]).map_err(|e|e.to_string())?;
        if n == 0 {
            return Err("delivery_held: delivery is no longer authorized for submission".into());
        }
        Ok(())
    }
    pub fn finish_message_delivery(
        &self,
        id: &str,
        state: &str,
        result: Option<&Value>,
        error: Option<&str>,
    ) -> Result<(), String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        // A queued dispatch can beat the admission ACK; accepted evidence wins.
        tx.execute("UPDATE agent_message_deliveries SET state=?2,result=COALESCE(?3,result),error=?4,
            queued_id=COALESCE(json_extract(?3,'$.queued_id'),queued_id),turn_id=COALESCE(json_extract(?3,'$.turn_id'),turn_id),
            retry_at=CASE WHEN ?2='pending' THEN datetime('now','+5 seconds') ELSE NULL END,updated_at=datetime('now')
            WHERE id=?1 AND state<>'cancelled' AND (?2 IN ('held','held_unknown','cancelled') OR (state NOT IN ('held','held_unknown') AND (state<>'accepted' OR ?2='accepted') AND NOT (state='pending' AND error IS NOT NULL AND ?2 IN ('accepted','submitting'))))",params![id,state,result.map(Value::to_string),error]).map_err(|e|e.to_string())?;
        tx.execute("UPDATE agent_control_operations SET
            state=CASE (SELECT state FROM agent_message_deliveries WHERE id=?1) WHEN 'accepted' THEN 'succeeded' WHEN 'queued' THEN 'succeeded' WHEN 'submitting' THEN 'succeeded' WHEN 'held_unknown' THEN 'needs_attention' WHEN 'held' THEN 'needs_attention' WHEN 'cancelled' THEN 'failed' WHEN 'rejected' THEN 'failed' ELSE 'accepted' END,
            dispatch_disposition=(SELECT state FROM agent_message_deliveries WHERE id=?1),
            result=CASE WHEN (SELECT result FROM agent_message_deliveries WHERE id=?1) IS NULL THEN result ELSE json_object('workspace_id',workspace_id,'thread_id',thread_id,'accepted_as',CASE WHEN (SELECT state FROM agent_message_deliveries WHERE id=?1)='queued' THEN 'queued' WHEN json_extract((SELECT result FROM agent_message_deliveries WHERE id=?1),'$.steered')=1 THEN 'steered' ELSE 'dispatched' END,'turn',json((SELECT result FROM agent_message_deliveries WHERE id=?1))) END,
            error=CASE WHEN (SELECT error FROM agent_message_deliveries WHERE id=?1) IS NULL THEN NULL ELSE json_object('code',(SELECT state FROM agent_message_deliveries WHERE id=?1),'message',(SELECT error FROM agent_message_deliveries WHERE id=?1)) END,
            updated_at=datetime('now') WHERE delivery_id=?1",
            params![id]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
    pub fn confirm_message_delivery_turn(&self, thread: &str, turn: &str) -> Result<(), String> {
        let ids: Vec<String> = {
            let conn = self.conn.lock().unwrap();
            let mut stmt=conn.prepare("SELECT id FROM agent_message_deliveries WHERE thread_id=?1 AND turn_id=?2 AND state IN ('submitting','held_unknown')").map_err(|e|e.to_string())?;
            let rows = stmt
                .query_map(params![thread, turn], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?
        };
        for id in ids {
            self.conn.lock().unwrap().execute("UPDATE agent_message_deliveries SET state='submitting' WHERE id=?1 AND state='held_unknown'",[&id]).map_err(|e|e.to_string())?;
            self.finish_message_delivery(&id, "accepted", None, None)?;
        }
        Ok(())
    }
    pub fn fail_message_delivery(&self, id: &str, error: &str) -> Result<String, String> {
        let record = self.message_delivery(id)?.ok_or("delivery_not_found")?;
        let state = if record.attempted {
            "held_unknown"
        } else {
            "pending"
        };
        self.finish_message_delivery(id, state, None, Some(error))?;
        Ok(state.into())
    }
    /// Interrupted preparation is definitely before send; submitting rows
    /// cannot be retried automatically. Provider queues remain manual on restart.
    pub fn message_delivery_recovery_attention(&self) -> Result<Vec<(String, String)>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt=conn.prepare("SELECT thread_id,error FROM agent_message_deliveries WHERE state='held_unknown' AND error IS NOT NULL ORDER BY updated_at DESC LIMIT 128").map_err(|e|e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }
    pub fn recover_message_deliveries(&self, epoch: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        // A durable exact-turn success can reconcile a lost runtime ACK.
        conn.execute("UPDATE agent_message_deliveries SET state='accepted',error=NULL WHERE state IN ('submitting','held_unknown') AND EXISTS (
            SELECT 1 FROM agent_chat_messages m WHERE m.thread_id=agent_message_deliveries.thread_id
            AND json_extract(m.payload,'$.type')='turn_completed' AND json_extract(m.payload,'$.turn_id')=agent_message_deliveries.turn_id
            AND json_extract(m.payload,'$.status.kind')='success')",[]).map_err(|e|e.to_string())?;
        conn.execute("UPDATE agent_control_operations SET state='succeeded',dispatch_disposition='accepted',error=NULL WHERE delivery_id IN (SELECT id FROM agent_message_deliveries WHERE state='accepted')",[]).map_err(|e|e.to_string())?;
        conn.execute("UPDATE agent_message_deliveries SET state=CASE WHEN attempted=1 THEN 'held_unknown' WHEN state='queued' THEN 'held' ELSE 'pending' END,
            error=CASE WHEN attempted=1 THEN 'Provider acceptance could not be confirmed after restart; automatic resend held' WHEN state='queued' THEN 'Saved queue awaits manual delivery' ELSE error END,
            epoch=?1,updated_at=datetime('now') WHERE epoch<>?1 AND state IN ('preparing','submitting','queued')",[epoch]).map_err(|e|e.to_string())?;
        conn.execute("UPDATE agent_control_operations SET state='needs_attention',dispatch_disposition=(SELECT state FROM agent_message_deliveries WHERE id=delivery_id),
            error=json_object('code','delivery_held','message',(SELECT error FROM agent_message_deliveries WHERE id=delivery_id))
            WHERE delivery_id IN (SELECT id FROM agent_message_deliveries WHERE state IN ('held','held_unknown'))",[]).map_err(|e|e.to_string())?;
        Ok(())
    }
    pub fn message_delivery_for_message(
        &self,
        thread: &str,
        message: &str,
    ) -> Result<Option<DeliveryRecord>, String> {
        self.conn.lock().unwrap().query_row(&format!("SELECT {COLUMNS} FROM agent_message_deliveries WHERE thread_id=?1 AND message_id=?2"),params![thread,message],read).optional().map_err(|e|e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_provider::message::*;
    use serde_json::json;
    fn admit(db: &DatabaseStore, id: &str) {
        db.admit_message_delivery(
            id,
            "receiver",
            id,
            &json!({"text":"review"}),
            "epoch-a",
            0,
            None,
            false,
        )
        .unwrap();
    }
    #[test]
    fn orchestration_message_migration_preserves_legacy_human_unknown() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE agent_chat_messages (id INTEGER PRIMARY KEY,thread_id TEXT,payload TEXT);
            CREATE TABLE agent_chat_queued_turns (thread_id TEXT,queued_id TEXT);
            CREATE TABLE agent_control_operations (id TEXT PRIMARY KEY);
            INSERT INTO agent_chat_messages VALUES (1,'legacy','{\"type\":\"user_message\",\"text\":\"[Codemux: delegated task results] user paste\"}');").unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        let fields: (Option<String>, String, String) = conn
            .query_row(
                "SELECT message_id,created_by,creation_source FROM agent_chat_messages",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(fields, (None, "user".into(), "unknown".into()));
        let legacy: MessageProvenance = serde_json::from_str(
            &conn
                .query_row::<String, _, _>("SELECT payload FROM agent_chat_messages", [], |r| {
                    r.get(0)
                })
                .unwrap(),
        )
        .unwrap();
        assert_eq!(legacy, MessageProvenance::default());
    }
    #[test]
    fn orchestration_message_identity_upserts_same_envelope_and_columns_after_restart() {
        let db = super::super::init_test_database();
        db.upsert_agent_chat_session("receiver", "workspace", None, "claude")
            .unwrap();
        let payload=json!({"type":"user_message","thread_id":"receiver","message_id":"stable","created_by":"agent","creation_source":"provider","sender_thread_id":"sender","sender_title":"Review agent","input_intent":"steer","text":"Visible brief","agent_text":"Visible brief\nAgent-only footer","notification":{"source":{"kind":"system","task_ids":[]},"outcome":"updated","summary":"Update"}}).to_string();
        let id = db
            .control_append_user_message("receiver", &payload, "turn", true)
            .unwrap()
            .unwrap();
        assert_eq!(
            db.control_append_user_message("receiver", &payload, "turn", true)
                .unwrap(),
            Some(id)
        );
        let root = tempfile::tempdir().unwrap();
        let reopened = db.control_fixture_snapshot(&root.path().join("messages.sqlite"));
        assert_eq!(
            reopened
                .control_append_user_message("receiver", &payload, "turn", true)
                .unwrap(),
            Some(id)
        );
        let conn = reopened.conn.lock().unwrap();
        let columns:(String,String,String,String,String)=conn.query_row("SELECT message_id,created_by,creation_source,sender_title,input_intent FROM agent_chat_messages WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).unwrap();
        assert_eq!(
            columns,
            (
                "stable".into(),
                "agent".into(),
                "provider".into(),
                "Review agent".into(),
                "steer".into()
            )
        );
        assert_eq!(
            conn.query_row::<i64, _, _>(
                "SELECT count(*) FROM agent_chat_messages WHERE message_id='stable'",
                [],
                |r| r.get(0)
            )
            .unwrap(),
            1
        );
    }
    #[test]
    fn orchestration_message_controller_admission_is_atomic_and_keyed_by_payload() {
        let db = super::super::init_test_database();
        let payload = json!({"input":{"text":"brief"},"provenance":{"created_by":"user","creation_source":"mcp"}});
        let operation = || {
            Some(DeliveryOperation {
                principal: "outside-client",
                key: "retry-key",
                hash: "hash",
                workspace: "workspace",
            })
        };
        assert_eq!(
            db.admit_message_delivery(
                "delivery",
                "receiver",
                "message",
                &payload,
                "epoch",
                0,
                operation(),
                false,
            )
            .unwrap(),
            ("delivery".into(), true)
        );
        assert_eq!(
            db.admit_message_delivery(
                "unused",
                "receiver",
                "unused",
                &payload,
                "epoch",
                0,
                operation(),
                false,
            )
            .unwrap(),
            ("delivery".into(), false)
        );
        let changed = Some(DeliveryOperation {
            principal: "outside-client",
            key: "retry-key",
            hash: "different",
            workspace: "workspace",
        });
        assert!(db
            .admit_message_delivery(
                "unused",
                "receiver",
                "unused",
                &json!({"text":"changed"}),
                "epoch",
                0,
                changed,
                false,
            )
            .unwrap_err()
            .starts_with("request_key_conflict"));
        let conn = db.conn.lock().unwrap();
        let stored: String = conn
            .query_row(
                "SELECT admitted_payload FROM agent_control_operations WHERE id='delivery'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&stored).unwrap(), payload);
        assert_eq!(
            conn.query_row::<i64, _, _>("SELECT count(*) FROM agent_message_deliveries", [], |r| r
                .get(0))
                .unwrap(),
            1
        );
    }
    #[test]
    fn orchestration_message_accept_then_drop_ack_holds_without_second_dispatch() {
        let db = super::super::init_test_database();
        admit(&db, "message");
        assert!(db.claim_message_delivery("message").unwrap());
        db.message_delivery_submitting("message").unwrap();
        // The adapter sent bytes, then lost its ACK/process. Reopen real rows.
        let root = tempfile::tempdir().unwrap();
        let reopened = db.control_fixture_snapshot(&root.path().join("unknown.sqlite"));
        reopened.recover_message_deliveries("epoch-b").unwrap();
        let row = reopened.message_delivery("message").unwrap().unwrap();
        assert_eq!(row.state, "held_unknown");
        assert!(row.attempted);
        assert!(!reopened.claim_message_delivery("message").unwrap());
        assert!(reopened.pending_message_deliveries().unwrap().is_empty());
        assert!(!reopened
            .thread_message_delivery_attention("receiver")
            .unwrap()
            .is_empty());
    }
    #[test]
    fn orchestration_message_definite_no_send_retries_same_attempt_and_identity() {
        let db = super::super::init_test_database();
        admit(&db, "message");
        assert!(db.claim_message_delivery("message").unwrap());
        assert_eq!(
            db.fail_message_delivery("message", "validation before provider bytes")
                .unwrap(),
            "pending"
        );
        let before = db.message_delivery("message").unwrap().unwrap();
        assert!(db.claim_message_delivery("message").unwrap());
        db.message_delivery_submitting("message").unwrap();
        db.finish_message_delivery(
            "message",
            "accepted",
            Some(&json!({"turn_id":"turn","queued_id":null,"steered":false})),
            None,
        )
        .unwrap();
        let after = db.message_delivery("message").unwrap().unwrap();
        assert_eq!(before.attempt_id, after.attempt_id);
        assert_eq!(before.message_id, after.message_id);
        db.recover_message_deliveries("epoch-b").unwrap();
        assert!(!db.claim_message_delivery("message").unwrap());
        assert_eq!(
            db.message_delivery("message").unwrap().unwrap().state,
            "accepted"
        );
    }
    #[test]
    fn orchestration_message_stop_generation_fences_admitted_and_queued_work() {
        let db = super::super::init_test_database();
        admit(&db, "message");
        db.claim_message_delivery("message").unwrap();
        db.finish_message_delivery(
            "message",
            "queued",
            Some(&json!({"turn_id":"","queued_id":"queue","steered":false})),
            None,
        )
        .unwrap();
        db.stop_message_deliveries("receiver").unwrap();
        assert!(db.verify_delivery_generation("receiver", 0).is_err());
        assert!(db.message_delivery_submitting("message").is_err());
        db.resume_message_delivery_thread("receiver").unwrap();
        assert!(db.verify_delivery_generation("receiver", 0).is_err());
        assert!(db.verify_delivery_generation("receiver", 1).is_ok());
        assert_eq!(
            db.message_delivery("message").unwrap().unwrap().state,
            "held"
        );
    }
    #[test]
    fn orchestration_message_envelope_existence_is_not_provider_acceptance() {
        let db = super::super::init_test_database();
        db.upsert_agent_chat_session("receiver", "workspace", None, "claude")
            .unwrap();
        admit(&db, "message");
        db.control_append_user_message("receiver",&json!({"type":"user_message","thread_id":"receiver","message_id":"message","delivery_id":"message","client_nonce":"nonce","text":"pre-dispatch"}).to_string(),"",false).unwrap();
        assert!(!db.queued_turn_was_sent("receiver", "nonce"));
        db.finish_message_delivery("message", "accepted", None, None)
            .unwrap();
        assert!(db.queued_turn_was_sent("receiver", "nonce"));
    }
    #[test]
    fn orchestration_message_late_queue_ack_cannot_replace_dispatch_receipt() {
        let db = super::super::init_test_database();
        db.admit_message_delivery(
            "delivery",
            "receiver",
            "message",
            &json!({"origin":"controller"}),
            "epoch",
            0,
            Some(DeliveryOperation {
                principal: "outside",
                key: "key",
                hash: "hash",
                workspace: "workspace",
            }),
            false,
        )
        .unwrap();
        db.claim_message_delivery("delivery").unwrap();
        db.finish_message_delivery(
            "delivery",
            "accepted",
            Some(&json!({"turn_id":"actual-turn","queued_id":null,"steered":true})),
            None,
        )
        .unwrap();
        db.finish_message_delivery(
            "delivery",
            "queued",
            Some(&json!({"turn_id":"","queued_id":"old-queue","steered":false})),
            None,
        )
        .unwrap();
        let receipt = db
            .control_operation("outside", "delivery", "epoch")
            .unwrap()
            .unwrap();
        assert_eq!(receipt.dispatch_disposition.as_deref(), Some("accepted"));
        assert_eq!(receipt.result.unwrap()["turn"]["turn_id"], "actual-turn");
    }
    #[test]
    fn orchestration_message_queue_edit_keeps_admission_and_retries_current_prompt() {
        let db = super::super::init_test_database();
        db.upsert_agent_chat_session("receiver", "workspace", None, "claude")
            .unwrap();
        db.admit_message_delivery(
            "delivery",
            "receiver",
            "message",
            &json!({"origin":"controller","input":{"text":"old prompt","display_text":"old"}}),
            "epoch",
            0,
            None,
            false,
        )
        .unwrap();
        db.claim_message_delivery("delivery").unwrap();
        db.finish_message_delivery(
            "delivery",
            "queued",
            Some(&json!({"turn_id":"","queued_id":"queue","steered":false})),
            None,
        )
        .unwrap();
        db.insert_queued_turn(&crate::database::queued_turns::QueuedTurnRecord {
            provenance: MessageProvenance {
                message_id: Some("message".into()),
                delivery_id: Some("delivery".into()),
                created_by: CreatedBy::Agent,
                ..Default::default()
            },
            thread_id: "receiver".into(),
            queued_id: "queue".into(),
            position: 0,
            text: "old".into(),
            prompt: Some("old prompt".into()),
            skill_ids: vec![],
            skill_text: None,
            include_plugins: false,
            images: vec![],
            client_nonce: Some("nonce".into()),
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            epoch: "epoch".into(),
        })
        .unwrap();
        db.update_queued_turn_text("receiver", "queue", "new", Some("new prompt"))
            .unwrap();
        let row = db.message_delivery("delivery").unwrap().unwrap();
        assert_eq!(row.payload["input"]["text"], "old prompt");
        assert_eq!(row.dispatch_payload.unwrap()["input"]["text"], "new prompt");
        assert_eq!(
            db.fail_message_delivery("delivery", "before bytes")
                .unwrap(),
            "pending"
        );
        assert_eq!(
            db.message_delivery("delivery").unwrap().unwrap().message_id,
            "message"
        );
    }
    #[test]
    fn orchestration_message_terminal_result_membership_has_one_delivery_owner() {
        let db = super::super::init_test_database();
        let payload = json!({"origin":"delegation","provenance":{"notification":{"source":{"kind":"delegated_task","task_ids":["task"]},"outcome":"completed"}}});
        db.admit_message_delivery(
            "delivery-a",
            "parent",
            "message-a",
            &payload,
            "epoch",
            0,
            None,
            false,
        )
        .unwrap();
        assert!(db
            .admit_message_delivery(
                "delivery-b",
                "parent",
                "message-b",
                &payload,
                "epoch",
                0,
                None,
                false
            )
            .unwrap_err()
            .starts_with("result_already_admitted"));
        assert!(
            db.message_delivery("delivery-b").unwrap().is_none(),
            "A conflicting result must roll back its entire admission"
        );
        db.admit_message_delivery(
            "delivery-c",
            "other-parent",
            "message-c",
            &payload,
            "epoch",
            0,
            None,
            false,
        )
        .unwrap();
    }
}
