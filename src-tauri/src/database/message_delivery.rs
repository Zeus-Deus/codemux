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
    ").map_err(|e| e.to_string())?;
    for (column, definition) in [("attempt_count", "INTEGER NOT NULL DEFAULT 0"), ("attention_acknowledged", "INTEGER NOT NULL DEFAULT 0"), ("origin_notified", "INTEGER NOT NULL DEFAULT 0")] {
        let present = conn.prepare("PRAGMA table_info(agent_message_deliveries)").map_err(|e| e.to_string())?
            .query_map([], |row| row.get::<_, String>(1)).map_err(|e| e.to_string())?
            .any(|name| name.is_ok_and(|name| name == column));
        if !present { conn.execute_batch(&format!("ALTER TABLE agent_message_deliveries ADD COLUMN {column} {definition}")).map_err(|e| e.to_string())?; }
    }
    Ok(())
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
    pub attempt_count: u32,
    pub origin_notified: bool,
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
        attempt_count: row.get(16)?,
        origin_notified: row.get(17)?,
    })
}
const COLUMNS: &str = "id,thread_id,message_id,payload,epoch,generation,attempt_id,state,principal,operation_id,queued_id,turn_id,result,error,attempted,dispatch_payload,attempt_count,origin_notified";

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
        let mut stmt=conn.prepare(&format!("SELECT {COLUMNS} FROM agent_message_deliveries WHERE thread_id=?1 AND state IN ('held','held_unknown','rejected') AND attention_acknowledged=0 AND COALESCE(json_extract(payload,'$.origin'),'controller')<>'user' ORDER BY created_at DESC LIMIT 20")).map_err(|e|e.to_string())?;
        let rows = stmt.query_map([thread], read).map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }
    pub fn inactive_delivery_queue_ids(&self, thread: &str) -> Result<Vec<String>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT queued_id FROM agent_message_deliveries WHERE thread_id=?1 AND queued_id IS NOT NULL AND state IN ('held','held_unknown','cancelled','rejected')").map_err(|e|e.to_string())?;
        let rows=stmt.query_map([thread],|r|r.get(0)).map_err(|e|e.to_string())?;
        rows.collect::<Result<_,_>>().map_err(|e|e.to_string())
    }
    pub fn rearm_saved_message_delivery(&self, id: &str, thread: &str, queued: &str) -> Result<(), String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e|e.to_string())?;
        let n=tx.execute("UPDATE agent_message_deliveries SET state='preparing',generation=COALESCE((SELECT generation FROM agent_thread_delivery_state WHERE thread_id=?2),0),error=NULL WHERE id=?1 AND thread_id=?2 AND queued_id=?3 AND attempted=0 AND state IN ('queued','held','rejected')",params![id,thread,queued]).map_err(|e|e.to_string())?;
        if n == 0 {
            return Err("Saved message identity no longer owns this queue row, or may already have reached the provider".into());
        }
        // The saved row remains until acceptance, but its old in-process queue
        // admission is retired before a replay can acquire a new queue ID.
        tx.execute("INSERT INTO agent_chat_messages(thread_id,payload) SELECT ?1,json_object('type','native_queue_control','thread_id',?1,'queued_id',?2,'disposition','cancelled','turn_id',NULL,'steered',json('false')) WHERE EXISTS (SELECT 1 FROM agent_chat_sessions WHERE thread_id=?1)",params![thread,queued]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e|e.to_string())
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
        let (current, _) = self.delivery_generation(thread)?;
        if current != generation {
            Err("delivery_held: thread was stopped after admission".into())
        } else {
            Ok(())
        }
    }
    /// An explicit user release re-arms only provider-owned queued work.
    /// Pending machine wakes disposed by Stop must never walk back in.
    pub fn resume_message_delivery_thread(&self, thread: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE agent_message_deliveries SET state=CASE WHEN queued_id IS NOT NULL THEN 'queued' WHEN json_extract(payload,'$.origin')='user' THEN 'preparing' ELSE 'pending' END,generation=COALESCE((SELECT generation FROM agent_thread_delivery_state WHERE thread_id=?1),0),error=NULL
            WHERE thread_id=?1 AND state='held' AND attempted=0 AND json_extract(payload,'$.origin') IN ('user','controller')", [thread]).map(drop).map_err(|e|e.to_string())
    }
    pub fn acknowledge_message_delivery(&self, thread: &str, id: &str) -> Result<(), String> {
        self.conn.lock().unwrap().execute("UPDATE agent_message_deliveries SET attention_acknowledged=1 WHERE thread_id=?1 AND id=?2 AND state IN ('held','held_unknown','rejected')", params![thread,id]).map(drop).map_err(|e|e.to_string())
    }
    /// Explicit retry is safe only when no provider submission was attempted.
    pub fn retry_message_delivery(&self, thread: &str, id: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute("UPDATE agent_message_deliveries SET state='pending',attempt_count=0,retry_at=NULL,error=NULL,attention_acknowledged=0,
            generation=COALESCE((SELECT generation FROM agent_thread_delivery_state WHERE thread_id=?1),0)
            WHERE thread_id=?1 AND id=?2 AND attempted=0 AND origin_notified=0 AND queued_id IS NULL AND state IN ('held','rejected')",params![thread,id]).map_err(|e|e.to_string())?;
        if n==0 { return Err("Delivery may already have reached the provider; automatic replay is held".into()); }
        Ok(())
    }
    pub fn stop_message_deliveries(&self, thread: &str) -> Result<(), String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute("INSERT INTO agent_thread_delivery_state (thread_id,generation,stopped) VALUES (?1,1,0) ON CONFLICT(thread_id) DO UPDATE SET generation=generation+1,stopped=0",[thread]).map_err(|e|e.to_string())?;
        tx.execute("UPDATE agent_message_deliveries SET state=CASE WHEN attempted=1 THEN CASE WHEN json_extract(payload,'$.origin')='user' THEN 'rejected' ELSE 'held_unknown' END WHEN json_extract(payload,'$.origin') IN ('user','controller') THEN 'held' ELSE 'cancelled' END,error='Thread stopped; explicit messages held and automatic wakes disposed' WHERE thread_id=?1 AND state IN ('pending','preparing','queued','submitting')",[thread]).map_err(|e|e.to_string())?;
        tx.execute("UPDATE agent_control_operations SET state=CASE (SELECT state FROM agent_message_deliveries WHERE id=delivery_id) WHEN 'cancelled' THEN 'failed' WHEN 'rejected' THEN 'failed' ELSE 'needs_attention' END,dispatch_disposition=(SELECT state FROM agent_message_deliveries WHERE id=delivery_id),error=json_object('code','delivery_held','message','Thread stopped') WHERE delivery_id IN (SELECT id FROM agent_message_deliveries WHERE thread_id=?1 AND state IN ('held','held_unknown','cancelled','rejected'))",[thread]).map_err(|e|e.to_string())?;
        retire_automatic_queues(&tx, Some(thread))?;
        tx.commit().map_err(|e| e.to_string())
    }

    pub fn disposed_delivery_queue_ids(&self, thread: &str) -> Result<Vec<String>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT queued_id FROM agent_message_deliveries WHERE thread_id=?1 AND state='cancelled' AND queued_id IS NOT NULL AND json_extract(payload,'$.origin') NOT IN ('user','controller')").map_err(|e|e.to_string())?;
        let rows = stmt.query_map([thread], |row| row.get(0)).map_err(|e|e.to_string())?;
        rows.collect::<Result<_,_>>().map_err(|e|e.to_string())
    }

    pub fn has_inflight_message_deliveries(&self, thread: &str) -> Result<bool, String> {
        self.conn.lock().unwrap().query_row("SELECT EXISTS(SELECT 1 FROM agent_message_deliveries d WHERE thread_id=?1 AND state IN ('pending','preparing','submitting') AND generation=COALESCE((SELECT generation FROM agent_thread_delivery_state WHERE thread_id=d.thread_id),0))", [thread], |r|r.get(0)).map_err(|e|e.to_string())
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
        tx.execute("INSERT INTO agent_message_deliveries (id,thread_id,message_id,payload,epoch,generation,attempt_id,principal,operation_id,state,attempt_count)
            VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![id,thread,message,payload.to_string(),epoch,generation,id,principal,operation_id,if claim_immediately {"preparing"}else{"pending"},u32::from(claim_immediately)]).map_err(|e|e.to_string())?;
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
    /// Hand definite no-send failures back to PR watch atomically. Its spent
    /// attempt stays spent, so rescheduling cannot bypass the watch budget.
    pub fn reconcile_pr_watch_deliveries(&self) -> Result<(), String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e|e.to_string())?;
        tx.execute("DELETE FROM pr_watch_seen WHERE EXISTS (
            SELECT 1 FROM agent_message_deliveries d JOIN pr_watches w ON w.thread_id=d.thread_id
            JOIN json_each(d.payload,'$.pr_watch.event_keys') k ON k.value=pr_watch_seen.event_key
            WHERE d.thread_id=pr_watch_seen.thread_id AND d.state='rejected' AND d.attempted=0 AND d.origin_notified=0
            AND json_extract(d.payload,'$.origin')='pr_watch' AND w.generation=json_extract(d.payload,'$.pr_watch.generation'))",[]).map_err(|e|e.to_string())?;
        tx.execute("UPDATE pr_watches SET last_error='Chat wake failed before submission; rescheduled within the watch attempt budget' WHERE EXISTS (
            SELECT 1 FROM agent_message_deliveries d WHERE d.thread_id=pr_watches.thread_id AND d.state='rejected' AND d.attempted=0 AND d.origin_notified=0
            AND json_extract(d.payload,'$.origin')='pr_watch' AND pr_watches.generation=json_extract(d.payload,'$.pr_watch.generation'))",[]).map_err(|e|e.to_string())?;
        tx.execute("UPDATE agent_message_deliveries SET origin_notified=1 WHERE state='rejected' AND attempted=0 AND json_extract(payload,'$.origin')='pr_watch'",[]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e|e.to_string())
    }
    pub fn pending_message_deliveries(&self) -> Result<Vec<DeliveryRecord>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM agent_message_deliveries WHERE state='pending' AND json_extract(payload,'$.origin')<>'user' AND generation=COALESCE((SELECT generation FROM agent_thread_delivery_state WHERE thread_id=agent_message_deliveries.thread_id),0) AND (retry_at IS NULL OR retry_at<=datetime('now')) ORDER BY created_at LIMIT 32")).map_err(|e|e.to_string())?;
        let rows = stmt.query_map([], read).map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    }
    pub fn claim_message_delivery(&self, id: &str) -> Result<bool, String> {
        self.conn.lock().unwrap().execute("UPDATE agent_message_deliveries SET state='preparing',attempt_count=attempt_count+1,updated_at=datetime('now') WHERE id=?1 AND state='pending'",[id]).map(|n|n==1).map_err(|e|e.to_string())
    }
    /// Called at the adapter's actual dispatch boundary, after all fallible
    /// authority checks. A crash from this point is possible acceptance.
    pub fn message_delivery_submitting(&self, id: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute("UPDATE agent_message_deliveries SET state=CASE WHEN state='accepted' THEN state ELSE 'submitting' END,attempted=1,updated_at=datetime('now') WHERE id=?1 AND state IN ('preparing','queued','submitting','accepted') AND generation=COALESCE((SELECT generation FROM agent_thread_delivery_state WHERE thread_id=agent_message_deliveries.thread_id),0)",[id]).map_err(|e|e.to_string())?;
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
        if state == "queued" {
            tx.execute("UPDATE agent_message_deliveries SET queued_id=COALESCE(json_extract(?2,'$.queued_id'),queued_id) WHERE id=?1 AND state='cancelled'", params![id,result.map(Value::to_string)]).map_err(|e|e.to_string())?;
        }
        tx.execute("UPDATE agent_message_deliveries SET state=CASE WHEN state='held' AND ?2='queued' THEN state ELSE ?2 END,result=COALESCE(?3,result),error=CASE WHEN state='held' AND ?2='queued' THEN error ELSE ?4 END,
            queued_id=COALESCE(json_extract(?3,'$.queued_id'),queued_id),turn_id=COALESCE(json_extract(?3,'$.turn_id'),turn_id),
            retry_at=CASE WHEN ?2='pending' THEN datetime('now','+' || MIN(300,5 * (1 << MIN(attempt_count,6))) || ' seconds') ELSE NULL END,updated_at=datetime('now')
            WHERE id=?1 AND state<>'cancelled' AND (state<>'accepted' OR ?2='accepted') AND ((state='held' AND ?2='queued') OR ?2 IN ('held','held_unknown','cancelled','accepted','submitting','rejected') OR (state NOT IN ('held','held_unknown','rejected') AND (state<>'accepted' OR ?2='accepted') AND NOT (state='pending' AND error IS NOT NULL AND ?2 IN ('accepted','submitting'))))",params![id,state,result.map(Value::to_string),error]).map_err(|e|e.to_string())?;
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
            let mut stmt=conn.prepare("SELECT id FROM agent_message_deliveries WHERE thread_id=?1 AND turn_id=?2 AND (state IN ('submitting','held_unknown') OR (state='rejected' AND attempted=1))").map_err(|e|e.to_string())?;
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
        if !matches!(record.state.as_str(), "preparing" | "submitting" | "queued") { return Ok(record.state); }
        let human = record.payload["origin"] == "user";
        let explicit_queue = record.queued_id.is_some() && matches!(record.payload["origin"].as_str(), Some("user"|"controller"));
        let state = if !record.attempted && explicit_queue { "held" }
            else if record.attempted && !human { "held_unknown" }
            else if record.attempted || human || record.attempt_count >= 3 { "rejected" }
            else { "pending" };
        self.finish_message_delivery(id, state, None, Some(error))?;
        Ok(state.into())
    }
    /// Interrupted preparation is definitely before send. Ordinary user queues
    /// remain manual; the machine completion outbox resumes independently.
    pub fn recover_message_deliveries(&self, epoch: &str) -> Result<(), String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e|e.to_string())?;
        let conn = &tx;
        // The envelope and native_user_control correlation commit atomically,
        // before the delivery ACK. Recover that identity even if turn_id is NULL.
        conn.execute("UPDATE agent_message_deliveries SET turn_id=COALESCE(NULLIF(turn_id,''),(
            SELECT json_extract(c.payload,'$.turn_id') FROM agent_chat_messages m JOIN agent_chat_messages c
            ON c.thread_id=m.thread_id AND json_extract(c.payload,'$.type')='native_user_control'
            AND json_extract(c.payload,'$.user_message_id')=m.id
            WHERE m.thread_id=agent_message_deliveries.thread_id AND m.message_id=agent_message_deliveries.message_id LIMIT 1))
            WHERE state IN ('submitting','held_unknown')",[]).map_err(|e|e.to_string())?;
        conn.execute("UPDATE agent_message_deliveries SET state='accepted',error=NULL WHERE state IN ('submitting','held_unknown') AND EXISTS (
            SELECT 1 FROM agent_chat_messages m WHERE m.thread_id=agent_message_deliveries.thread_id
            AND json_extract(m.payload,'$.type')='turn_completed' AND json_extract(m.payload,'$.turn_id')=agent_message_deliveries.turn_id)",[]).map_err(|e|e.to_string())?;
        // For native ACK adapters the correlated envelope is written after
        // actual acceptance. ACP worker ACKs alone remain ambiguous.
        conn.execute("UPDATE agent_message_deliveries SET state='accepted',error=NULL WHERE state IN ('submitting','held_unknown')
            AND json_extract(payload,'$.provider') IN ('claude','codex','opencode') AND EXISTS (
                SELECT 1 FROM agent_chat_messages m JOIN agent_chat_messages c ON c.thread_id=m.thread_id
                AND json_extract(c.payload,'$.type')='native_user_control' AND json_extract(c.payload,'$.user_message_id')=m.id
                WHERE m.thread_id=agent_message_deliveries.thread_id AND m.message_id=agent_message_deliveries.message_id
                AND json_extract(c.payload,'$.turn_id')=agent_message_deliveries.turn_id AND agent_message_deliveries.turn_id<>'')",[]).map_err(|e|e.to_string())?;
        conn.execute("UPDATE agent_control_operations SET state='succeeded',dispatch_disposition='accepted',error=NULL WHERE delivery_id IN (SELECT id FROM agent_message_deliveries WHERE state='accepted')",[]).map_err(|e|e.to_string())?;
        conn.execute("UPDATE agent_message_deliveries SET state=CASE WHEN attempted=1 THEN CASE WHEN json_extract(payload,'$.origin')='user' THEN 'rejected' ELSE 'held_unknown' END WHEN (queued_id IS NOT NULL AND json_extract(payload,'$.origin') IN ('user','controller')) OR json_extract(payload,'$.origin')='user' THEN 'held' ELSE 'pending' END,
            error=CASE WHEN attempted=1 THEN 'Provider acceptance could not be confirmed after restart; automatic resend held' WHEN queued_id IS NOT NULL AND json_extract(payload,'$.origin') IN ('user','controller') THEN 'Saved queue awaits manual delivery' ELSE NULL END,
            epoch=?1,updated_at=datetime('now') WHERE epoch<>?1 AND state IN ('preparing','submitting','queued')",[epoch]).map_err(|e|e.to_string())?;
        conn.execute("UPDATE agent_control_operations SET state='needs_attention',dispatch_disposition=(SELECT state FROM agent_message_deliveries WHERE id=delivery_id),
            error=json_object('code','delivery_held','message',(SELECT error FROM agent_message_deliveries WHERE id=delivery_id))
            WHERE delivery_id IN (SELECT id FROM agent_message_deliveries WHERE state IN ('held','held_unknown'))",[]).map_err(|e|e.to_string())?;
        retire_automatic_queues(conn, None)?;
        // The worker owns recovered wakes; old queue IDs must never identify
        // their next attempt, nor authorize a saved-row replay.
        conn.execute("UPDATE agent_message_deliveries SET queued_id=NULL,result=NULL WHERE state='pending' AND json_extract(payload,'$.origin') NOT IN ('user','controller')", []).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e|e.to_string())
    }

    pub fn message_delivery_for_message(
        &self,
        thread: &str,
        message: &str,
    ) -> Result<Option<DeliveryRecord>, String> {
        self.conn.lock().unwrap().query_row(&format!("SELECT {COLUMNS} FROM agent_message_deliveries WHERE thread_id=?1 AND message_id=?2"),params![thread,message],read).optional().map_err(|e|e.to_string())
    }
}

impl DeliveryRecord {
    pub fn can_retry(&self) -> bool {
        !self.attempted && !self.origin_notified && self.queued_id.is_none()
            && matches!(self.state.as_str(), "held" | "rejected")
    }
}

/// Retire only automatic wake queue ownership, atomically with its journal
/// transition. Explicit messages retain their saved row and original identity.
fn retire_automatic_queues(conn: &Connection, thread: Option<&str>) -> Result<(), String> {
    conn.execute("INSERT INTO agent_chat_messages(thread_id,payload)
        SELECT d.thread_id,json_object('type','native_queue_control','thread_id',d.thread_id,'queued_id',d.queued_id,'disposition','cancelled','turn_id',NULL,'steered',json('false'))
        FROM agent_message_deliveries d WHERE d.state IN ('pending','cancelled') AND d.queued_id IS NOT NULL
        AND json_extract(d.payload,'$.origin') NOT IN ('user','controller') AND (?1 IS NULL OR d.thread_id=?1)
        AND EXISTS (SELECT 1 FROM agent_chat_sessions s WHERE s.thread_id=d.thread_id)
        AND NOT EXISTS (SELECT 1 FROM agent_chat_messages m WHERE m.thread_id=d.thread_id AND json_extract(m.payload,'$.type')='native_queue_control'
            AND json_extract(m.payload,'$.queued_id')=d.queued_id AND json_extract(m.payload,'$.disposition') IN ('dispatched','cancelled'))", [thread]).map_err(|e|e.to_string())?;
    conn.execute("DELETE FROM agent_chat_queued_turns WHERE EXISTS (SELECT 1 FROM agent_message_deliveries d WHERE d.thread_id=agent_chat_queued_turns.thread_id AND d.queued_id=agent_chat_queued_turns.queued_id AND d.state IN ('pending','cancelled') AND json_extract(d.payload,'$.origin') NOT IN ('user','controller') AND (?1 IS NULL OR d.thread_id=?1))", [thread]).map_err(|e|e.to_string())?;
    Ok(())
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
            &json!({"origin":"controller","text":"review"}),
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
        db.admit_message_delivery("message","receiver","message",&json!({"origin":"user"}),"epoch-a",0,None,false).unwrap();
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
            "queued"
        );
        db.message_delivery_submitting("message").unwrap();
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
            "held"
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
    #[test]
    fn orchestration_message_stop_disposes_old_machine_intent_without_blocking_new_admission() {
        let db = super::super::init_test_database();
        db.admit_message_delivery("old","receiver","old",&json!({"origin":"pr_watch"}),"epoch-a",0,None,false).unwrap();
        db.stop_message_deliveries("receiver").unwrap();
        assert_eq!(db.message_delivery("old").unwrap().unwrap().state,"cancelled");
        assert!(db.verify_delivery_generation("receiver",1).is_ok());
        db.admit_message_delivery("new","receiver","new",&json!({"origin":"controller"}),"epoch-a",1,None,false).unwrap();
        db.claim_message_delivery("new").unwrap();
        db.message_delivery_submitting("new").unwrap();
        db.resume_message_delivery_thread("receiver").unwrap();
        assert!(!db.claim_message_delivery("old").unwrap());
    }

    #[test]
    fn orchestration_message_real_dispatch_evidence_resolves_held_rows() {
        let db=super::super::init_test_database();
        admit(&db,"message");
        db.finish_message_delivery("message","held",None,Some("held before ACK")).unwrap();
        db.finish_message_delivery("message","accepted",Some(&json!({"turn_id":"actual"})),None).unwrap();
        assert_eq!(db.message_delivery("message").unwrap().unwrap().state,"accepted");
    }

    #[test]
    fn orchestration_message_human_stop_and_restart_never_need_machine_attention() {
        let db=super::super::init_test_database();
        for id in ["stopped","failed"] {
            db.admit_message_delivery(id,"receiver",id,&json!({"origin":"user"}),"epoch-a",0,None,true).unwrap();
            db.message_delivery_submitting(id).unwrap();
        }
        db.fail_message_delivery("failed","adapter error").unwrap();
        db.finish_message_delivery("stopped","submitting",Some(&json!({"turn_id":"turn"})),None).unwrap();
        db.stop_message_deliveries("receiver").unwrap();
        db.admit_message_delivery("restarted","receiver","restarted",&json!({"origin":"user"}),"epoch-a",1,None,true).unwrap();
        db.message_delivery_submitting("restarted").unwrap();
        db.recover_message_deliveries("epoch-b").unwrap();
        assert_eq!(db.message_delivery("restarted").unwrap().unwrap().state,"rejected");
        assert!(db.thread_message_delivery_attention("receiver").unwrap().is_empty());
        db.confirm_message_delivery_turn("receiver","turn").unwrap();
        assert_eq!(db.message_delivery("stopped").unwrap().unwrap().state,"accepted");
    }

    #[test]
    fn orchestration_message_attention_acknowledgement_survives_restart_without_resending() {
        let db=super::super::init_test_database();
        admit(&db,"message"); db.claim_message_delivery("message").unwrap();
        db.message_delivery_submitting("message").unwrap();
        db.fail_message_delivery("message","lost ACK").unwrap();
        assert_eq!(db.thread_message_delivery_attention("receiver").unwrap().len(),1);
        db.acknowledge_message_delivery("receiver","message").unwrap();
        let root=tempfile::tempdir().unwrap();
        let reopened=db.control_fixture_snapshot(&root.path().join("attention.sqlite"));
        reopened.recover_message_deliveries("epoch-b").unwrap();
        assert!(reopened.thread_message_delivery_attention("receiver").unwrap().is_empty());
        assert!(reopened.retry_message_delivery("receiver","message").is_err());
        assert!(!reopened.claim_message_delivery("message").unwrap());
    }

    #[test]
    fn orchestration_message_retries_back_off_and_stop_after_three_pre_send_failures() {
        let db=super::super::init_test_database(); admit(&db,"message");
        let mut previous=0;
        for attempt in 1..=3 {
            assert!(db.claim_message_delivery("message").unwrap());
            let state=db.fail_message_delivery("message","provider disabled").unwrap();
            assert_eq!(db.message_delivery("message").unwrap().unwrap().attempt_count,attempt);
            if attempt<3 {
                assert_eq!(state,"pending");
                let seconds:i64=db.conn.lock().unwrap().query_row("SELECT unixepoch(retry_at)-unixepoch('now') FROM agent_message_deliveries WHERE id='message'",[],|r|r.get(0)).unwrap();
                assert!(seconds>previous); previous=seconds;
                assert!(db.pending_message_deliveries().unwrap().is_empty());
            } else { assert_eq!(state,"rejected"); }
        }
        assert!(!db.claim_message_delivery("message").unwrap());
        db.retry_message_delivery("receiver","message").unwrap();
        assert!(db.claim_message_delivery("message").unwrap());
        db.message_delivery_submitting("message").unwrap();
        assert_eq!(db.message_delivery("message").unwrap().unwrap().message_id,"message");
    }

    #[test]
    fn orchestration_message_pr_watch_terminal_failure_reschedules_within_spent_budget() {
        let db=super::super::init_test_database();
        db.upsert_agent_chat_session("receiver","workspace",None,"claude").unwrap();
        let mut watch=db.arm_pr_watch("receiver","workspace",1,Some("https://example.test/a/b/pull/1"),2,1).unwrap();
        watch.attempts=1;
        db.save_pr_watch_progress(&watch,&["check:ci".into()],2).unwrap();
        let payload=json!({"origin":"pr_watch","pr_watch":{"generation":watch.generation,"event_keys":["check:ci"]}});
        db.admit_message_delivery("wake","receiver","wake",&payload,"epoch-a",0,None,true).unwrap();
        db.finish_message_delivery("wake","rejected",None,Some("missing provider")).unwrap();
        db.reconcile_pr_watch_deliveries().unwrap();
        assert!(db.pr_watch_seen_keys("receiver").unwrap().is_empty());
        let after=db.get_pr_watch("receiver").unwrap().unwrap();
        assert_eq!(after.attempts,1); assert!(after.last_error.is_some());
        db.save_pr_watch_progress(&after,&["check:ci".into()],3).unwrap();
        db.reconcile_pr_watch_deliveries().unwrap();
        assert!(db.pr_watch_seen_keys("receiver").unwrap().contains("check:ci"));
    }

    #[test]
    fn orchestration_message_machine_completion_queue_recovers_independently_of_manual_queue() {
        let db=super::super::init_test_database();
        db.upsert_agent_chat_session("receiver","workspace",None,"claude").unwrap();
        db.control_record_queue_admission("receiver","provider-queue").unwrap();
        let payload=json!({"origin":"delegation","provenance":{"notification":{"source":{"kind":"delegated_task","task_ids":["task"]},"outcome":"completed"}}});
        db.admit_message_delivery("result","receiver","result",&payload,"epoch-a",0,None,true).unwrap();
        db.finish_message_delivery("result","queued",Some(&json!({"queued_id":"provider-queue"})),None).unwrap();
        db.admit_message_delivery("human","receiver","human",&json!({"origin":"user"}),"epoch-a",0,None,true).unwrap();
        db.finish_message_delivery("human","queued",Some(&json!({"queued_id":"human-queue"})),None).unwrap();
        let root=tempfile::tempdir().unwrap();
        let db=db.control_fixture_snapshot(&root.path().join("completion-outbox.sqlite"));
        db.recover_message_deliveries("epoch-b").unwrap();
        assert!(db.control_unresolved_queued_ids("receiver").unwrap().is_empty());
        assert_eq!(db.message_delivery("human").unwrap().unwrap().state,"held");
        assert_eq!(db.pending_message_deliveries().unwrap().iter().map(|r|r.id.as_str()).collect::<Vec<_>>(),vec!["result"]);
        assert!(db.claim_message_delivery("result").unwrap());
        assert!(db.admit_message_delivery("duplicate","receiver","duplicate",&payload,"epoch-b",0,None,false).is_err());
    }

    #[test]
    fn orchestration_message_recovery_correlates_lost_delivery_ack_via_message_id() {
        let db=super::super::init_test_database();
        db.upsert_agent_chat_session("receiver","workspace",None,"claude").unwrap();
        admit(&db,"message"); db.claim_message_delivery("message").unwrap(); db.message_delivery_submitting("message").unwrap();
        db.control_append_user_message("receiver",&json!({"type":"user_message","thread_id":"receiver","message_id":"message","text":"brief"}).to_string(),"actual-turn",false).unwrap();
        db.append_agent_chat_message("receiver",&json!({"type":"turn_completed","thread_id":"receiver","turn_id":"actual-turn","status":{"kind":"error","subtype":"interrupted"}}).to_string()).unwrap();
        assert!(db.message_delivery("message").unwrap().unwrap().turn_id.is_none());
        for missing in [None,Some("")] {
            db.conn.lock().unwrap().execute("UPDATE agent_message_deliveries SET turn_id=?1,state='submitting' WHERE id='message'",[missing]).unwrap();
            db.recover_message_deliveries("epoch-b").unwrap();
            let row=db.message_delivery("message").unwrap().unwrap();
            assert_eq!(row.turn_id.as_deref(),Some("actual-turn")); assert_eq!(row.state,"accepted");
        }
    }

    #[test]
    fn orchestration_message_stop_before_queue_ack_preserves_manual_release() {
        for release_first in [false,true] {
            let db=super::super::init_test_database();
            db.admit_message_delivery("b","receiver","b",&json!({"origin":"user"}),"epoch",0,None,true).unwrap();
            db.stop_message_deliveries("receiver").unwrap();
            if release_first { db.resume_message_delivery_thread("receiver").unwrap(); }
            db.finish_message_delivery("b","queued",Some(&json!({"queued_id":"b-queue"})),None).unwrap();
            if !release_first {
                assert_eq!(db.message_delivery("b").unwrap().unwrap().state,"held");
                db.resume_message_delivery_thread("receiver").unwrap();
            }
            db.message_delivery_submitting("b").unwrap();
            assert_eq!(db.message_delivery("b").unwrap().unwrap().generation,1);
        }
    }

    #[test]
    fn orchestration_message_recovery_distinguishes_native_ack_from_deferred_worker_ack() {
        for provider in ["claude","codex","opencode","acp"] {
            let db=super::super::init_test_database();
            db.upsert_agent_chat_session("receiver","workspace",None,provider).unwrap();
            db.admit_message_delivery("message","receiver","message",&json!({"provider":provider,"origin":"controller"}),"epoch-a",0,None,true).unwrap();
            db.message_delivery_submitting("message").unwrap();
            db.control_append_user_message("receiver",&json!({"type":"user_message","thread_id":"receiver","message_id":"message","text":"brief"}).to_string(),"native-ack",false).unwrap();
            db.recover_message_deliveries("epoch-b").unwrap();
            assert_eq!(db.message_delivery("message").unwrap().unwrap().state,if provider=="acp" {"held_unknown"} else {"accepted"});
        }
    }

    fn saved(db: &DatabaseStore, id: &str, origin: &str, queued: &str) {
        db.admit_message_delivery(id,"receiver",id,&json!({"origin":origin,"provider":"claude","input":{"thread_id":"receiver","text":"message","delivery":"queue"}}),"epoch-a",0,None,true).unwrap();
        db.finish_message_delivery(id,"queued",Some(&json!({"queued_id":queued})),None).unwrap();
        db.control_record_queue_admission("receiver",queued).unwrap();
        db.insert_queued_turn(&crate::database::queued_turns::QueuedTurnRecord {
            thread_id:"receiver".into(),queued_id:queued.into(),position:0,text:"message".into(),prompt:Some("message".into()),
            provenance:MessageProvenance {message_id:Some(id.into()),delivery_id:Some(id.into()),input_intent:InputIntent::QueuedTurn,..Default::default()},
            skill_ids:vec![],skill_text:None,include_plugins:false,images:vec![],client_nonce:Some(id.into()),
            model_override:None,effort_override:None,permission_mode_override:None,epoch:"epoch-a".into(),
        }).unwrap();
    }

    #[test]
    fn orchestration_message_stop_holds_all_explicit_messages_and_disposes_only_wakes() {
        let db=super::super::init_test_database();
        db.upsert_agent_chat_session("receiver","workspace",None,"claude").unwrap();
        for origin in ["user","controller","delegation","remote_delegation","usage_resume","restart_resume","pr_watch"] {
            saved(&db,origin,origin,&format!("queue-{origin}"));
        }
        db.stop_message_deliveries("receiver").unwrap();
        for origin in ["user","controller"] {
            assert_eq!(db.message_delivery(origin).unwrap().unwrap().state,"held");
            assert!(db.get_queued_turn("receiver",&format!("queue-{origin}")).is_some());
            assert!(db.message_delivery_submitting(origin).is_err());
        }
        for origin in ["delegation","remote_delegation","usage_resume","restart_resume","pr_watch"] {
            assert_eq!(db.message_delivery(origin).unwrap().unwrap().state,"cancelled");
            assert!(db.get_queued_turn("receiver",&format!("queue-{origin}")).is_none());
            assert!(!db.control_unresolved_queued_ids("receiver").unwrap().contains(&format!("queue-{origin}")));
        }
        db.resume_message_delivery_thread("receiver").unwrap();
        for origin in ["user","controller"] { db.message_delivery_submitting(origin).unwrap(); }
        assert!(!db.claim_message_delivery("pr_watch").unwrap());
    }

    #[test]
    fn orchestration_message_restart_keeps_explicit_queue_identity_without_worker_replay() {
        let db=super::super::init_test_database();
        db.upsert_agent_chat_session("receiver","workspace",None,"claude").unwrap();
        for origin in ["controller","user"] { saved(&db,origin,origin,&format!("old-{origin}")); }
        let root=tempfile::tempdir().unwrap();
        let db=db.control_fixture_snapshot(&root.path().join("explicit.sqlite"));
        db.recover_message_deliveries("epoch-b").unwrap();
        assert!(db.pending_message_deliveries().unwrap().is_empty());
        for origin in ["controller","user"] {
            let row=db.message_delivery(origin).unwrap().unwrap();
            assert_eq!(row.state,"held");assert_eq!(row.queued_id.as_deref(),Some(format!("old-{origin}").as_str()));
            assert_eq!(db.get_queued_turn("receiver",&format!("old-{origin}")).unwrap().provenance.message_id.as_deref(),Some(origin));
        }
        assert!(db.rearm_saved_message_delivery("controller","receiver","different-queue").is_err());
        db.rearm_saved_message_delivery("controller","receiver","old-controller").unwrap();
        db.finish_message_delivery("controller","queued",Some(&json!({"queued_id":"new-controller"})),None).unwrap();
        assert!(!db.control_unresolved_queued_ids("receiver").unwrap().contains(&"old-controller".into()));
        assert!(db.rearm_saved_message_delivery("controller","receiver","old-controller").is_err());
        db.message_delivery_submitting("controller").unwrap();
        db.finish_message_delivery("controller","accepted",Some(&json!({"turn_id":"actual"})),None).unwrap();
        assert!(db.rearm_saved_message_delivery("controller","receiver","old-controller").is_err());
        assert!(!db.control_unresolved_queued_ids("receiver").unwrap().contains(&"old-controller".into()));
    }

    #[test]
    fn orchestration_message_restart_recovers_every_automatic_origin_and_clears_old_ownership() {
        let db=super::super::init_test_database();
        db.upsert_agent_chat_session("receiver","workspace",None,"claude").unwrap();
        for origin in ["delegation","remote_delegation","usage_resume","restart_resume","pr_watch"] { saved(&db,origin,origin,&format!("old-{origin}")); }
        db.recover_message_deliveries("epoch-b").unwrap();
        assert_eq!(db.pending_message_deliveries().unwrap().len(),5);
        assert!(db.list_queued_turns("receiver").is_empty());
        assert!(db.control_unresolved_queued_ids("receiver").unwrap().is_empty());
        for row in db.pending_message_deliveries().unwrap() {
            assert!(row.queued_id.is_none());assert!(row.result.is_none());
            assert!(db.rearm_saved_message_delivery(&row.id,"receiver",&format!("old-{}",row.id)).is_err());
        }
        db.recover_message_deliveries("epoch-b").unwrap();
        assert_eq!(db.pending_message_deliveries().unwrap().len(),5);
    }

    #[test]
    fn orchestration_message_reconciled_pr_watch_wake_cannot_be_retried() {
        let db=super::super::init_test_database();
        db.admit_message_delivery("wake","receiver","wake",&json!({"origin":"pr_watch"}),"epoch-a",0,None,true).unwrap();
        db.finish_message_delivery("wake","rejected",None,Some("before send")).unwrap();
        assert!(db.message_delivery("wake").unwrap().unwrap().can_retry());
        db.reconcile_pr_watch_deliveries().unwrap();
        assert!(!db.message_delivery("wake").unwrap().unwrap().can_retry());
        assert!(db.retry_message_delivery("receiver","wake").is_err());
        assert!(!db.claim_message_delivery("wake").unwrap());
    }

    #[test]
    fn orchestration_message_late_disposed_queue_ack_retains_cleanup_identity() {
        let db=super::super::init_test_database();
        db.admit_message_delivery("wake","receiver","wake",&json!({"origin":"pr_watch"}),"epoch-a",0,None,true).unwrap();
        db.stop_message_deliveries("receiver").unwrap();
        db.finish_message_delivery("wake","queued",Some(&json!({"queued_id":"late-queue"})),None).unwrap();
        let row=db.message_delivery("wake").unwrap().unwrap();
        assert_eq!(row.state,"cancelled");assert_eq!(row.queued_id.as_deref(),Some("late-queue"));
        assert_eq!(db.disposed_delivery_queue_ids("receiver").unwrap(),vec!["late-queue"]);
    }

}
