use rusqlite::{params, Connection, OptionalExtension};
use std::{path::Path, sync::Mutex};

pub struct Journal {
    pub(super) conn: Mutex<Connection>,
    dispatch_wakers: Mutex<Vec<std::sync::Weak<futures_util::task::AtomicWaker>>>,
}
impl Journal {
    pub fn open(root: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
        let path = root.join("journal.sqlite3");
        let conn = Connection::open(&path).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| e.to_string())?;
        }
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
          CREATE TABLE IF NOT EXISTS identities(parent TEXT NOT NULL, nonce TEXT NOT NULL, input TEXT NOT NULL, id TEXT NOT NULL UNIQUE, PRIMARY KEY(parent,nonce));
          CREATE TABLE IF NOT EXISTS grants(id TEXT PRIMARY KEY, scope TEXT NOT NULL, data TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS tasks(id TEXT PRIMARY KEY, data TEXT NOT NULL, request TEXT NOT NULL, cursor INTEGER NOT NULL DEFAULT 0, launched INTEGER NOT NULL DEFAULT 0);
          CREATE TABLE IF NOT EXISTS host_info(host INTEGER PRIMARY KEY, data TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS attempts(task TEXT PRIMARY KEY);
          CREATE TABLE IF NOT EXISTS launch_outcomes(task TEXT PRIMARY KEY, state TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS delivery_outcomes(task TEXT PRIMARY KEY, attempt TEXT NOT NULL, state TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS remote_pages(task TEXT PRIMARY KEY, pending INTEGER NOT NULL);
          CREATE TABLE IF NOT EXISTS ceilings(task TEXT PRIMARY KEY, mode TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS response_outcomes(task TEXT NOT NULL, request TEXT NOT NULL, decision TEXT NOT NULL, state TEXT NOT NULL, PRIMARY KEY(task,request));
          CREATE TABLE IF NOT EXISTS responses(task TEXT NOT NULL, request TEXT NOT NULL, PRIMARY KEY(task,request));
          CREATE TABLE IF NOT EXISTS events(task TEXT NOT NULL, sequence INTEGER NOT NULL, data TEXT NOT NULL, PRIMARY KEY(task,sequence));")
          .map_err(|e|e.to_string())?;
        conn.execute_batch("CREATE TABLE IF NOT EXISTS event_storage(singleton INTEGER PRIMARY KEY CHECK(singleton=1), bytes INTEGER NOT NULL);
            INSERT OR IGNORE INTO event_storage SELECT 1,COALESCE(SUM(length(CAST(data AS BLOB))),0) FROM events;
            CREATE TRIGGER IF NOT EXISTS bound_delegation_event_storage BEFORE INSERT ON events
            WHEN NOT EXISTS(SELECT 1 FROM events WHERE task=NEW.task AND sequence=NEW.sequence)
            BEGIN SELECT CASE WHEN (SELECT bytes FROM event_storage WHERE singleton=1)+length(CAST(NEW.data AS BLOB))>536870912
            THEN RAISE(ABORT,'Delegation global event retention limit reached; saved cursor/history retained') END; END;
            CREATE TRIGGER IF NOT EXISTS count_delegation_event_storage AFTER INSERT ON events
            BEGIN UPDATE event_storage SET bytes=bytes+length(CAST(NEW.data AS BLOB)) WHERE singleton=1; END;").map_err(|e| e.to_string())?;
        // Existing rows deliberately remain NULL: current metadata is not
        // evidence of the request the user originally consented to.
        let bound:bool=conn.prepare("PRAGMA table_info(response_outcomes)").map_err(|e|e.to_string())?.query_map([],|r|r.get::<_,String>(1)).map_err(|e|e.to_string())?.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())?.iter().any(|name|name=="original_request");
        if !bound {conn.execute_batch("ALTER TABLE response_outcomes ADD COLUMN original_request TEXT").map_err(|e|e.to_string())?;}
        Ok(Self {
            conn: Mutex::new(conn),
            dispatch_wakers: Mutex::new(Vec::new()),
        })
    }
    pub(crate) fn register_dispatch(
        &self,
        waker: &std::sync::Arc<futures_util::task::AtomicWaker>,
    ) {
        let mut wakers = self.dispatch_wakers.lock().unwrap();
        wakers.retain(|w| w.strong_count() > 0);
        wakers.push(std::sync::Arc::downgrade(waker));
    }
    fn wake_dispatches(&self) {
        let mut wakers = self.dispatch_wakers.lock().unwrap();
        wakers.retain(|w| w.strong_count() > 0);
        for w in wakers.iter().filter_map(std::sync::Weak::upgrade) {
            w.wake();
        }
    }
    pub fn reserve(&self, parent: &str, nonce: &str, input: &str) -> Result<String, String> {
        reserve(&self.conn.lock().unwrap(), parent, nonce, input)
    }
}
pub(super) fn reserve(
    conn: &Connection,
    parent: &str,
    nonce: &str,
    input: &str,
) -> Result<String, String> {
    if nonce.is_empty() || nonce.len() > 200 || parent.is_empty() {
        return Err("invalid client request identity".into());
    }
    let existing: Option<(String, String)> = conn
        .query_row(
            "SELECT input,id FROM identities WHERE parent=?1 AND nonce=?2",
            params![parent, nonce],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some((prior, id)) = existing {
        if prior != input {
            return Err("client_request_id conflicts with a different task request".into());
        }
        return Ok(id);
    }
    // Exact retries above remain valid even after retention reaches its bound.
    let retained: i64 = conn.query_row("SELECT COUNT(*) FROM identities", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    if retained >= 2048 || input.len() > 65536 || parent.len() > 512 {
        return Err("Delegation intent retention/admission limit reached; existing identities and history are preserved (no automatic pruning)".into());
    }
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO identities(parent,nonce,input,id) VALUES (?1,?2,?3,?4)",
        params![parent, nonce, input, id],
    )
    .map_err(|e| e.to_string())?;
    Ok(id)
}

impl Journal {
    pub(crate) fn poll_wake(
        &self,
        id: &str,
        attempt: &str,
        validate: impl FnOnce(&LocalTask, &Grant) -> Result<(), String>,
        write: &mut dyn FnMut() -> crate::json_rpc_child::dispatch::WritePoll,
    ) -> Result<crate::json_rpc_child::dispatch::WritePoll, String> {
        let conn = self.conn.lock().unwrap();
        let task = read_task(&conn, id)?;
        if task.cancel_requested || task.wake_state != WakeState::Delivering {
            return Err("Result delivery superseded".into());
        }
        let input: String = conn
            .query_row("SELECT input FROM identities WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .map_err(|e| e.to_string())?;
        let target = decode::<DelegateTaskInput>(&input)?.target_id;
        let data: String = conn
            .query_row(
                "SELECT data FROM grants WHERE id=?1 AND scope=?2",
                params![target, task.parent_workspace_id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        let grant: Grant = decode(&data)?;
        task.validate_target(&grant)?;
        validate(&task, &grant)?;
        let state: Option<String> = conn
            .query_row(
                "SELECT state FROM delivery_outcomes WHERE task=?1 AND attempt=?2",
                params![id, attempt],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if state.as_deref() != Some("in_flight") {
            return Err("Result delivery attempt is not admissible".into());
        }
        // Commit Unknown BEFORE a syscall can accept any bytes. A crash in
        // this interval cannot safely prove non-submission. Pending proves
        // no bytes, so restore in_flight while retaining the same mutex.
        conn.execute(
            "UPDATE delivery_outcomes SET state='unknown' WHERE task=?1 AND attempt=?2",
            params![id, attempt],
        )
        .map_err(|e| e.to_string())?;
        let poll = write();
        if poll.is_pending() {
            conn.execute(
                "UPDATE delivery_outcomes SET state='in_flight' WHERE task=?1 AND attempt=?2",
                params![id, attempt],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(poll)
    }
    pub fn cache_host(
        &self,
        id: i64,
        info: &crate::remote::tasks::TaskCapabilities,
    ) -> Result<(), String> {
        let data = encode(info)?;
        let conn = self.conn.lock().unwrap();
        let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM host_info WHERE host=?1)", [id], |r| r.get(0)).map_err(|e| e.to_string())?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM host_info", [], |r| r.get(0)).map_err(|e| e.to_string())?;
        if data.len() > 2 * 1024 * 1024 || (!exists && count >= 128) { return Err("Delegation capability cache capacity reached; existing host cache retained".into()); }
        conn.execute("INSERT INTO host_info(host,data) VALUES(?1,?2) ON CONFLICT(host) DO UPDATE SET data=excluded.data",params![id,data]).map_err(|e|e.to_string())?;
        Ok(())
    }
    pub fn host_info(&self, id: i64) -> Result<crate::remote::tasks::TaskCapabilities, String> {
        let data: String = self
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT data FROM host_info WHERE host=?1", [id], |r| {
                r.get(0)
            })
            .map_err(|_| {
                "Select the host in Run on host to probe its native capabilities first".to_string()
            })?;
        decode(&data)
    }
    pub fn attempted(&self, id: &str) -> Result<bool, String> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM attempts WHERE task=?1)",
                [id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())
    }
    pub fn mark_attempted(&self, id: &str) -> Result<(), String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        if read_task(&tx, id)?.cancel_requested {
            return Err("Stopped intent cannot begin a Launch".into());
        }
        tx.execute("INSERT OR IGNORE INTO attempts(task) VALUES(?1)", [id])
            .map_err(|e| e.to_string())?;
        tx.execute("INSERT INTO launch_outcomes(task,state) VALUES(?1,'in_flight') ON CONFLICT(task) DO UPDATE SET state='in_flight' WHERE state!='fenced'",[id]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
    pub fn finish_launch(
        &self,
        id: &str,
        outcome: &Result<serde_json::Value, super::transport::TransportError>,
    ) -> Result<(), String> {
        let state = match outcome {
            Ok(_) => "accepted",
            Err(super::transport::TransportError::BeforeSend(_)) => "before_send",
            Err(_) => "unknown",
        };
        self.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE launch_outcomes SET state=?2 WHERE task=?1 AND state!='fenced'",
                params![id, state],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn settle_fenced_absence(&self, id: &str) -> Result<(), String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let mut task = read_task(&tx, id)?;
        if !task.cancel_requested || task.remote.is_some() {
            return Err("Unexpected receiver absence fence for an admitted/live intent".into());
        }
        task.status = crate::remote::tasks::TaskStatus::Cancelled;
        task.connection_error = None;
        task.updated_at = chrono::Utc::now().to_rfc3339();
        tx.execute(
            "UPDATE tasks SET data=?2 WHERE id=?1",
            params![id, encode(&task)?],
        )
        .map_err(|e| e.to_string())?;
        tx.execute("INSERT INTO launch_outcomes(task,state) VALUES(?1,'fenced') ON CONFLICT(task) DO UPDATE SET state='fenced'",[id]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
    /// Persist an immutable decision before transport. Only a known before-
    /// send failure can dispatch it again; in-flight/unknown attempts reconcile.
    pub fn claim_response(
        &self,
        id: &str,
        response: &crate::remote::tasks::RespondRequest,
    ) -> Result<bool, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let task=read_task(&tx,id)?;
        if task.cancel_requested {
            return Err("Stopped task cannot dispatch an approval".into());
        }
        let decision = encode(&response.decision)?;
        let prior: Option<(String, String, Option<String>)> = tx
            .query_row(
                "SELECT decision,state,original_request FROM response_outcomes WHERE task=?1 AND request=?2",
                params![id, response.request_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let binding=response.original_request.as_ref().ok_or("Original approval consent identity is missing")?;
        let encoded_binding=encode(binding)?;
        if binding.request_id!=response.request_id {return Err("Original approval request identity conflicts".into());}
        if let Some((previous, state, original)) = prior {
            if previous != decision {
                return Err("Approval conflicts with the immutable durable decision".into());
            }
            if original.as_deref()!=Some(&encoded_binding) {return Err("Original durable approval identity is missing or changed".into());}
            if state != "before_send" {
                return Ok(false);
            }
            if !task.remote.as_ref().is_some_and(|t|!t.cancel_requested && !t.status.is_terminal() && t.pending_requests.iter().any(|r|r==binding)) {return Err("Original approval payload/kind changed".into());}
            tx.execute(
                "UPDATE response_outcomes SET state='in_flight' WHERE task=?1 AND request=?2",
                params![id, response.request_id],
            )
            .map_err(|e| e.to_string())?;
        } else {
            let count: i64 = tx.query_row("SELECT COUNT(*) FROM response_outcomes WHERE task=?1", [id], |r| r.get(0)).map_err(|e| e.to_string())?;
            if count >= 256 || decision.len() > 65536 || encoded_binding.len()>65536 || response.request_id.len() > 512 {
                return Err("Delegation approval retention/admission capacity reached; prior decisions remain immutable".into());
            }
            let legacy: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM responses WHERE task=?1 AND request=?2)",
                    params![id, response.request_id],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if legacy {
                return Err(
                    "Legacy approval attempt has unknown decision/outcome; do not replay".into(),
                );
            }
            if !task.remote.as_ref().is_some_and(|t|!t.cancel_requested && !t.status.is_terminal() && t.pending_requests.iter().any(|r|r==binding)) {return Err("Original approval payload/kind changed".into());}
            tx.execute(
                "INSERT INTO responses(task,request) VALUES(?1,?2)",
                params![id, response.request_id],
            )
            .map_err(|e| e.to_string())?;
            tx.execute("INSERT INTO response_outcomes(task,request,decision,state,original_request) VALUES(?1,?2,?3,'in_flight',?4)",params![id,response.request_id,decision,encoded_binding]).map_err(|e|e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(true)
    }
    pub fn finish_response(
        &self,
        id: &str,
        request: &str,
        outcome: &Result<serde_json::Value, super::transport::TransportError>,
    ) -> Result<(), String> {
        let state = match outcome {
            Ok(_) => "accepted",
            Err(super::transport::TransportError::BeforeSend(_)) => "before_send",
            Err(_) => "unknown",
        };
        self.conn.lock().unwrap().execute("UPDATE response_outcomes SET state=?3 WHERE task=?1 AND request=?2 AND state!='accepted'",params![id,request,state]).map_err(|e|e.to_string())?;
        Ok(())
    }
    pub fn grant_for_task(&self, id: &str) -> Result<Grant, String> {
        let conn = self.conn.lock().unwrap();
        let target: String = conn
            .query_row("SELECT input FROM identities WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .map_err(|e| e.to_string())?;
        let target = decode::<DelegateTaskInput>(&target)?.target_id;
        let task = read_task(&conn, id)?;
        let data: String = conn
            .query_row(
                "SELECT data FROM grants WHERE id=?1 AND scope=?2",
                params![target, task.parent_workspace_id],
                |r| r.get(0),
            )
            .map_err(|_| "Delegation grant is missing".to_string())?;
        decode(&data)
    }
}

use super::types::*;
use crate::remote::tasks::{LaunchRequest, TaskRead as RemoteRead};
fn encode<T: serde::Serialize>(v: &T) -> Result<String, String> {
    serde_json::to_string(v).map_err(|e| e.to_string())
}
fn decode<T: serde::de::DeserializeOwned>(v: &str) -> Result<T, String> {
    serde_json::from_str(v).map_err(|e| e.to_string())
}
impl Journal {
    pub fn put_grant(&self, scope: &str, grant: &Grant) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM grants WHERE id=?1)", [&grant.id], |r| r.get(0)).map_err(|e| e.to_string())?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM grants", [], |r| r.get(0)).map_err(|e| e.to_string())?;
        if (!exists && count >= 128) || encode(grant)?.len() > 16384 {
            return Err("Delegation grant retention limit reached; existing grants can still be revoked".into());
        }
        conn.execute("INSERT INTO grants(id,scope,data) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET data=excluded.data WHERE scope=excluded.scope",params![grant.id,scope,encode(grant)?]).map_err(|e|e.to_string())?;
        drop(conn);
        self.wake_dispatches();
        Ok(())
    }
    /// Disable only the retained row in its original scope. No caller-supplied
    /// metadata is accepted and enabled-grant write quotas are unchanged.
    pub fn disable_grant(&self, scope: &str, id: &str) -> Result<(),String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|e|e.to_string())?;
        let data: String = tx.query_row("SELECT data FROM grants WHERE id=?1 AND scope=?2",params![id,scope],|r|r.get(0)).map_err(|_|"Grant does not belong to this workspace".to_string())?;
        let mut retained: serde_json::Value = decode(&data)?;
        retained.as_object_mut().ok_or("Invalid retained grant")?.insert("enabled".into(),serde_json::Value::Bool(false));
        tx.execute("UPDATE grants SET data=?3 WHERE id=?1 AND scope=?2",params![id,scope,encode(&retained)?]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e|e.to_string())?;
        drop(conn);
        self.wake_dispatches();
        Ok(())
    }
    pub fn grants(&self, scope: &str) -> Result<Vec<Grant>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT data FROM grants WHERE scope=?1 ORDER BY id")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([scope], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.map(|r| decode(&r.map_err(|e| e.to_string())?))
            .collect()
    }
    pub fn admit(
        &self,
        parent: &Parent,
        grant: &Grant,
        workspace_id: &str,
        input: &DelegateTaskInput,
    ) -> Result<LocalTask, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        // Consent lookup and intent insertion share this transaction. A
        // cloned enabled grant cannot cross a concurrent revoke/scope change.
        let durable: String = tx
            .query_row(
                "SELECT data FROM grants WHERE id=?1 AND scope=?2",
                params![grant.id, parent.workspace_id],
                |r| r.get(0),
            )
            .map_err(|_| "Delegation grant is absent from the original workspace".to_string())?;
        if !grant.enabled || durable != encode(grant)? {
            return Err("Delegation grant changed or was revoked before durable admission".into());
        }
        let id = reserve(
            &tx,
            &parent.thread_id,
            &input.client_request_id,
            &encode(input)?,
        )?;
        let prior: Option<String> = tx
            .query_row("SELECT data FROM tasks WHERE id=?1", [&id], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(data) = prior {
            let task: LocalTask = decode(&data)?;
            if task.parent_workspace_id != parent.workspace_id
                || task.parent_provider != parent.provider
            {
                return Err("Original task parent ownership cannot be rebound".into());
            }
            return Ok(task);
        }
        let task = LocalTask::new(id, parent, grant, workspace_id.into(), input);
        tx.execute(
            "INSERT INTO tasks(id,data,request) VALUES(?1,?2,?3)",
            params![
                task.id,
                encode(&task)?,
                encode(&task.request(input.effort.clone()))?
            ],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO ceilings(task,mode) VALUES(?1,?2)",
            params![task.id, parent.permission_mode],
        )
        .map_err(|e| e.to_string())?;
        // Prospective provenance, committed with first admission before any
        // effect. Never backfill legacy identities from current presentation.
        // The empty attempt denotes no dispatch claim; claim_delivery replaces
        // it atomically with the actual attempt and in_flight outcome.
        tx.execute(
            "INSERT INTO delivery_outcomes(task,attempt,state) VALUES(?1,'','not_attempted')",
            [&task.id],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(task)
    }
    pub fn task(&self, id: &str) -> Result<LocalTask, String> {
        let conn = self.conn.lock().unwrap();
        read_task(&conn, id)
    }
    pub fn tasks(&self) -> Result<Vec<LocalTask>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT data FROM tasks ORDER BY rowid")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.map(|r| decode(&r.map_err(|e| e.to_string())?))
            .collect()
    }
    pub fn update(&self, id: &str, f: impl FnOnce(&mut LocalTask)) -> Result<LocalTask, String> {
        let conn = self.conn.lock().unwrap();
        let mut task = read_task(&conn, id)?;
        f(&mut task);
        task.updated_at = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE tasks SET data=?2 WHERE id=?1",
            params![id, encode(&task)?],
        )
        .map_err(|e| e.to_string())?;
        Ok(task)
    }
    pub fn follow_intent(&self, id: &str) -> Result<(LaunchRequest, i64, bool), String> {
        let conn = self.conn.lock().unwrap();
        let (request, cursor, launched): (String, i64, bool) = conn
            .query_row(
                "SELECT request,cursor,launched FROM tasks WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(|e| e.to_string())?;
        Ok((decode(&request)?, cursor, launched))
    }
    /// Launch/Cancel/Respond receipts always contain the page from zero.
    pub fn apply_remote(&self, id: &str, read: &RemoteRead) -> Result<(), String> {
        self.apply_remote_after(id, read, 0)
    }
    /// Read pages are bound to the literal cursor sent, not a inferred first ID.
    pub fn apply_remote_after(&self, id: &str, read: &RemoteRead, after: i64) -> Result<(), String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let mut task = read_task(&tx, id)?;
        let (request, cursor): (String, i64) = tx
            .query_row("SELECT request,cursor FROM tasks WHERE id=?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .map_err(|e| e.to_string())?;
        let fenced: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM launch_outcomes WHERE task=?1 AND state='fenced')",
                [id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if fenced {
            return Err(
                "Late task receipt contradicted acknowledged receiver cancellation fence".into(),
            );
        }
        if read.task.id != id || read.task.request != decode::<LaunchRequest>(&request)? {
            return Err("remote task identity/request does not match durable intent".into());
        }
        if after < 0 || after > cursor || (after != 0 && after != cursor) {
            return Err("remote page does not match the durable requested cursor".into());
        }
        let incoming_time = chrono::DateTime::parse_from_rfc3339(&read.task.updated_at).map_err(|_| "remote snapshot timestamp is malformed")?;
        if let Some(prior) = task.remote.as_ref() {
            let prior_time = chrono::DateTime::parse_from_rfc3339(&prior.updated_at).map_err(|_| "cached remote snapshot timestamp is malformed")?;
            if incoming_time < prior_time { return Err("stale remote snapshot cannot regress cached progress/result".into()); }
        }
        if read.task.child_thread_id.is_empty() || read.task.turn_id.as_ref().is_some_and(String::is_empty) || task.remote.as_ref().is_some_and(|prior| {
            prior.child_thread_id != read.task.child_thread_id
                || prior.turn_id.as_ref().is_some_and(|turn| read.task.turn_id.as_ref() != Some(turn))
        }) {
            return Err("remote child/accepted turn identity changed".into());
        }
        let mut expected = after;
        for event in &read.events {
            expected = expected.checked_add(1).ok_or("remote sequence overflow")?;
            if event.sequence != expected || event.event.get("thread_id").and_then(|v| v.as_str()).is_some_and(|thread| thread != read.task.child_thread_id) {
                return Err("remote page is noncontiguous or belongs to another child".into());
            }
        }
        if read.next_cursor != expected || (read.has_more && expected == after)
            || (expected < cursor && (!read.has_more || read.events.is_empty())) {
            return Err("remote page cursor skipped events, regressed or made no progress".into());
        }
        if let Some(prior) = task.remote.as_ref().filter(|t| t.status.is_terminal()) {
            if encode(prior)? != encode(&read.task)? {
                return Err("remote attempted to mutate an immutable terminal result".into());
            }
        }
        for receipt in &read.approval_receipts {
            let prior: Option<(String,Option<String>)> = tx
                .query_row(
                    "SELECT decision,original_request FROM response_outcomes WHERE task=?1 AND request=?2",
                    params![id, receipt.request_id],
                    |r| Ok((r.get(0)?,r.get(1)?)),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            if let Some((prior,binding)) = prior {
                if prior != encode(&receipt.decision)? {
                    return Err("Receiver approval receipt conflicts with durable decision".into());
                }
                // A legacy receipt cannot prove new consent, or upgrade an
                // unknown attempt using metadata sampled after consent.
                if binding.is_none() || receipt.original_request.as_ref().map(encode).transpose()?!=binding {continue;}
                tx.execute(
                    "UPDATE response_outcomes SET state='accepted' WHERE task=?1 AND request=?2",
                    params![id, receipt.request_id],
                )
                .map_err(|e| e.to_string())?;
            }
        }
        if read.events.len() > 100 || encode(read)?.len() > 2 * 1024 * 1024 {
            return Err("Remote page exceeds the bounded wire contract; saved cursor/history retained".into());
        }
        let (retained_count, retained_bytes): (i64, i64) = tx.query_row(
            "SELECT COUNT(*),COALESCE(SUM(length(CAST(data AS BLOB))),0) FROM events WHERE task=?1", [id], |r| Ok((r.get(0)?, r.get(1)?))
        ).map_err(|e| e.to_string())?;
        let mut added_count = 0i64;
        let mut added_bytes = 0i64;
        let mut previous = 0;
        for event in &read.events {
            if event.sequence <= previous || event.sequence > read.next_cursor {
                return Err("invalid remote event cursor".into());
            }
            previous = event.sequence;
            let prior: Option<String> = tx
                .query_row(
                    "SELECT data FROM events WHERE task=?1 AND sequence=?2",
                    params![id, event.sequence],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            let encoded = encode(&event.event)?;
            if prior.as_ref().is_some_and(|v| v != &encoded) {
                return Err("remote event changed under a stable sequence".into());
            }
            if prior.is_none() {
                added_count += 1; added_bytes += encoded.len() as i64;
                if retained_count + added_count > 10000 || retained_bytes + added_bytes > 64 * 1024 * 1024 {
                    return Err("Delegation event retention limit reached; saved cursor/history retained and receiver tail not acknowledged".into());
                }
            }
            tx.execute(
                "INSERT OR IGNORE INTO events(task,sequence,data) VALUES(?1,?2,?3)",
                params![id, event.sequence, encoded],
            )
            .map_err(|e| e.to_string())?;
        }
        if read.next_cursor < previous || read.next_cursor < 0 {
            return Err("invalid remote cursor".into());
        }
        task.status = if task.cancel_requested && !read.task.status.is_terminal() {
            crate::remote::tasks::TaskStatus::Stopping
        } else {
            read.task.status
        };
        task.remote = Some(read.task.clone());
        task.connection_error = None;
        task.updated_at = chrono::Utc::now().to_rfc3339();
        tx.execute(
            "UPDATE tasks SET data=?2,cursor=?3,launched=1 WHERE id=?1",
            params![id, encode(&task)?, cursor.max(read.next_cursor)],
        )
        .map_err(|e| e.to_string())?;
        tx.execute("INSERT INTO remote_pages(task,pending) VALUES(?1,?2) ON CONFLICT(task) DO UPDATE SET pending=excluded.pending",params![id,read.has_more]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
    pub fn read(&self, id: &str, cursor: i64) -> Result<TaskRead, String> {
        if cursor < 0 {
            return Err("cursor must be nonnegative".into());
        }
        let conn = self.conn.lock().unwrap();
        let task = read_task(&conn, id)?;
        let mut stmt=conn.prepare("SELECT sequence,data FROM events WHERE task=?1 AND sequence>?2 ORDER BY sequence LIMIT 101").map_err(|e|e.to_string())?;
        let rows = stmt
            .query_map(params![id, cursor], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?;
        let mut events = Vec::new();
        let mut bytes = 0;
        let mut has_more = false;
        for row in rows {
            let (sequence, data) = row.map_err(|e| e.to_string())?;
            if events.len() == 100 || (bytes + data.len() > 1024 * 1024 && !events.is_empty()) {
                has_more = true;
                break;
            }
            bytes += data.len();
            events.push(crate::remote::tasks::TaskEvent {
                sequence,
                event: decode(&data)?,
            });
        }
        let next_cursor = events.last().map(|e| e.sequence).unwrap_or(cursor);
        Ok(TaskRead {
            task,
            events,
            next_cursor,
            has_more,
        })
    }
    pub fn tail_pending(&self, id: &str) -> Result<bool, String> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT pending FROM remote_pages WHERE task=?1",
                [id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())
            .map(|v| v.unwrap_or(false))
    }
    pub fn ceiling(&self, id: &str) -> Result<String, String> {
        self.conn
            .lock()
            .unwrap()
            .query_row("SELECT mode FROM ceilings WHERE task=?1", [id], |r| {
                r.get(0)
            })
            .map_err(|_| "Parent dispatch ceiling is missing".into())
    }
    pub(crate) fn response_outcome(&self, id:&str, request:&str)->Result<(DeliveryOutcome,Option<crate::agent_provider::ApprovalDecision>,Option<crate::remote::tasks::RemoteApproval>),String> {
        let conn=self.conn.lock().unwrap();
        let prior:Option<(String,String,Option<String>)>=conn.query_row("SELECT state,decision,original_request FROM response_outcomes WHERE task=?1 AND request=?2",params![id,request],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|e|e.to_string())?;
        if let Some((state,decision,binding))=prior {
            let decision=serde_json::from_str(&decision).ok();
            let binding=binding.and_then(|b|serde_json::from_str::<crate::remote::tasks::RemoteApproval>(&b).ok()).filter(|b|b.request_id==request);
            let outcome=if decision.is_some() {match state.as_str() {"accepted"=>DeliveryOutcome::Accepted,"before_send" if binding.is_some()=>DeliveryOutcome::BeforeSend,"in_flight" if binding.is_some()=>DeliveryOutcome::InFlight,_=>DeliveryOutcome::Unknown}} else {DeliveryOutcome::Unknown};
            return Ok((outcome,decision,binding));
        }
        let legacy:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM responses WHERE task=?1 AND request=?2)",params![id,request],|r|r.get(0)).map_err(|e|e.to_string())?;
        Ok((if legacy {DeliveryOutcome::Unknown} else {DeliveryOutcome::NotAttempted},None,None))
    }
    pub(crate) fn delivery_outcome(&self, id:&str)->Result<DeliveryOutcome,String> {
        let conn=self.conn.lock().unwrap();
        let state:Option<String>=conn.query_row("SELECT state FROM delivery_outcomes WHERE task=?1",[id],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
        state.map(|s|serde_json::from_value(serde_json::Value::String(s)).map_err(|e|e.to_string())).unwrap_or(Ok(DeliveryOutcome::NotAttempted))
    }
    pub(crate) fn claim_delivery(&self, id: &str) -> Result<Option<String>, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let mut task = read_task(&tx, id)?;
        if task.cancel_requested || task.wake_state != WakeState::Pending {
            return Ok(None);
        }
        let prior: Option<String> = tx
            .query_row(
                "SELECT state FROM delivery_outcomes WHERE task=?1",
                [id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if prior.as_deref().is_some_and(|s| !matches!(s, "before_send" | "not_attempted")) {
            return Err(
                "Result was accepted or may have been submitted; explicit resend is forbidden"
                    .into(),
            );
        }
        let attempt = uuid::Uuid::new_v4().to_string();
        tx.execute("INSERT INTO delivery_outcomes(task,attempt,state) VALUES(?1,?2,'in_flight') ON CONFLICT(task) DO UPDATE SET attempt=excluded.attempt,state='in_flight'",params![id,attempt]).map_err(|e|e.to_string())?;
        task.wake_state = WakeState::Delivering;
        tx.execute(
            "UPDATE tasks SET data=?2 WHERE id=?1",
            params![id, encode(&task)?],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(Some(attempt))
    }
    pub(crate) fn delivery_attempt(&self, id: &str) -> Result<String, String> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT attempt FROM delivery_outcomes WHERE task=?1 AND state='in_flight'",
                [id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())
    }
    pub(crate) fn accept_delivery(&self, id: &str, attempt: &str) -> Result<(), String> {
        let n=self.conn.lock().unwrap().execute("UPDATE delivery_outcomes SET state='accepted' WHERE task=?1 AND attempt=?2 AND state='unknown'",params![id,attempt]).map_err(|e|e.to_string())?;
        if n != 1 {
            return Err("Missing correlated native delivery attempt".into());
        }
        Ok(())
    }
    pub(crate) fn finish_delivery(&self, id: &str, attempt: &str) -> Result<(), String> {
        // Only an attempt that never crossed a possible-byte poll can become
        // before_send. Unknown/Accepted evidence survives every presentation.
        self.conn.lock().unwrap().execute("UPDATE delivery_outcomes SET state='before_send' WHERE task=?1 AND attempt=?2 AND state='in_flight'",params![id,attempt]).map_err(|e|e.to_string())?;
        Ok(())
    }
    pub(crate) fn explicit_delivery(&self, id: &str) -> Result<(), String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let mut task = read_task(&tx, id)?;
        let state: Option<String> = tx
            .query_row(
                "SELECT state FROM delivery_outcomes WHERE task=?1",
                [id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if state.as_deref().is_some_and(|s| !matches!(s, "before_send" | "not_attempted"))
            || task.cancel_requested
            || matches!(
                task.wake_state,
                WakeState::Delivered | WakeState::Delivering
            )
        {
            return Err(
                "Result was accepted or may have been submitted; explicit resend is forbidden"
                    .into(),
            );
        }
        task.wake_state = WakeState::Pending;
        task.wake_error = None;
        tx.execute(
            "UPDATE tasks SET data=?2 WHERE id=?1",
            params![id, encode(&task)?],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
    pub fn recover_deliveries(&self) -> Result<(), String> {
        self.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE delivery_outcomes SET state='unknown' WHERE state='in_flight'",
                [],
            )
            .map_err(|e| e.to_string())?;
        for task in self.tasks()? {
            if matches!(
                task.wake_state,
                WakeState::Delivering | WakeState::Held | WakeState::Suppressed
            ) {
                // Older versions had presentation but no native byte/ack ledger.
                // Absence of evidence is not permission to replay such a result.
                // Explicit modern not_attempted rows survive INSERT OR IGNORE;
                // cancellation, missing output or parent availability alone
                // must never downgrade an existing unknown/accepted outcome.
                self.conn.lock().unwrap().execute("INSERT OR IGNORE INTO delivery_outcomes(task,attempt,state) VALUES(?1,?2,'unknown')",params![task.id,uuid::Uuid::new_v4().to_string()]).map_err(|e|e.to_string())?;
            }
            if task.wake_state == WakeState::Delivering {
                self.update(&task.id,|t|{t.wake_state=WakeState::Held;t.wake_error=Some("Desktop restarted during delivery; native outcome is unknown and resending is forbidden".into());})?;
            }
        }
        Ok(())
    }
    pub fn cancel(&self, id: &str) -> Result<LocalTask, String> {
        let task = self.update(id, |t| {
            t.cancel_requested = true;
            t.wake_state = WakeState::Suppressed;
            if !t.status.is_terminal() {
                t.status = crate::remote::tasks::TaskStatus::Stopping;
            }
        })?;
        self.wake_dispatches();
        Ok(task)
    }
    pub fn suppress_parent(&self, parent: &str, stop: bool) -> Result<(), String> {
        for task in self
            .tasks()?
            .into_iter()
            .filter(|t| t.parent_thread_id == parent && t.wake_state != WakeState::Delivered)
        {
            self.update(&task.id, |t| {
                t.wake_state = WakeState::Suppressed;
                t.wake_error =
                    Some("Parent user activity superseded automatic result delivery".into());
                if stop && !t.status.is_terminal() {
                    t.cancel_requested = true;
                    t.status = crate::remote::tasks::TaskStatus::Stopping;
                }
            })?;
        }
        Ok(())
    }
}
fn read_task(conn: &Connection, id: &str) -> Result<LocalTask, String> {
    let data: String = conn
        .query_row("SELECT data FROM tasks WHERE id=?1", [id], |r| r.get(0))
        .map_err(|_| "unknown delegated task".to_string())?;
    decode(&data)
}

impl Journal {
    pub(crate) fn poll_transport(
        &self, id:&str, response:Option<&crate::remote::tasks::RespondRequest>,
        validate:impl FnOnce(&LocalTask,&Grant,&str)->Result<(),String>,
        write:&mut dyn FnMut()->crate::json_rpc_child::dispatch::WritePoll,
    )->Result<crate::json_rpc_child::dispatch::WritePoll,String> {
        let conn=self.conn.lock().unwrap();
        let task=read_task(&conn,id)?;
        if task.cancel_requested || task.status.is_terminal() {return Err("Stopped/settled task cannot dispatch remote effects".into());}
        let input:String=conn.query_row("SELECT input FROM identities WHERE id=?1",[id],|r|r.get(0)).map_err(|e|e.to_string())?;
        let target=decode::<DelegateTaskInput>(&input)?.target_id;
        let data:String=conn.query_row("SELECT data FROM grants WHERE id=?1 AND scope=?2",params![target,task.parent_workspace_id],|r|r.get(0)).map_err(|_|"Grant is absent from the original workspace".to_string())?;
        let grant:Grant=decode(&data)?;
        task.validate_target(&grant)?;
        let ceiling:String=conn.query_row("SELECT mode FROM ceilings WHERE task=?1",[id],|r|r.get(0)).map_err(|e|e.to_string())?;
        if let Some(response)=response {
            if !task.remote.as_ref().is_some_and(|t|!t.status.is_terminal() && !t.cancel_requested && t.pending_requests.iter().any(|r|r.request_id==response.request_id)) {return Err("Approval is no longer pending".into());}
            let (decision,state,original):(String,String,Option<String>)=conn.query_row("SELECT decision,state,original_request FROM response_outcomes WHERE task=?1 AND request=?2",params![id,response.request_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|e|e.to_string())?;
            let binding=response.original_request.as_ref().ok_or("Original consent identity missing at dispatch")?;
            if state!="in_flight" || decision!=encode(&response.decision)? || original.as_deref()!=Some(&encode(binding)?) || binding.request_id!=response.request_id || !task.remote.as_ref().is_some_and(|t|t.pending_requests.iter().any(|r|r==binding)) {return Err("Immutable approval kind/payload/decision is no longer admissible".into());}
        } else {
            let state:String=conn.query_row("SELECT state FROM launch_outcomes WHERE task=?1",[id],|r|r.get(0)).map_err(|e|e.to_string())?;
            if state!="in_flight" {return Err("Launch attempt is no longer admissible".into());}
        }
        validate(&task,&grant,&ceiling)?;
        Ok(write())
    }
}
