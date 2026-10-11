use super::{LaunchRequest, RespondRequest, TaskEvent, TaskRead, TaskSnapshot, TaskStatus};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use std::os::unix::{
    fs::{DirBuilderExt, OpenOptionsExt},
    io::AsRawFd,
};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct TaskStore {
    conn: Mutex<Connection>,
    root: PathBuf,
}
impl TaskStore {
    pub fn snapshot(&self, id: &str) -> Result<TaskSnapshot, String> {
        validate_id(id)?;
        let conn = self.conn.lock().map_err(db_error)?;
        load(&conn, id)
    }
    pub fn existing(&self, request: &LaunchRequest) -> Result<Option<TaskSnapshot>, String> {
        validate_id(&request.id)?;
        let conn = self.conn.lock().map_err(db_error)?;
        let found = conn
            .query_row(
                "SELECT request FROM tasks WHERE id=?1",
                [&request.id],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(db_error)?;
        match found {
            None => Ok(None),
            Some(existing) if existing == serde_json::to_string(request).map_err(db_error)? => {
                Ok(Some(load(&conn, &request.id)?))
            }
            Some(_) => Err("task id conflicts with a different launch request".into()),
        }
    }
    pub fn read(&self, id: &str, after: i64) -> Result<TaskRead, String> {
        if after < 0 {
            return Err("event cursor must be nonnegative".into());
        }
        validate_id(id)?;
        let mut conn = self.conn.lock().map_err(db_error)?;
        let tx = conn.transaction().map_err(db_error)?;
        let task = load(&tx, id)?;
        let mut statement = tx.prepare("SELECT sequence,event FROM task_events WHERE task_id=?1 AND sequence>?2 ORDER BY sequence LIMIT 101").map_err(db_error)?;
        let mut rows = statement
            .query(rusqlite::params![id, after])
            .map_err(db_error)?;
        let mut events = vec![];
        let mut bytes = 2;
        let mut has_more = false;
        while let Some(row) = rows.next().map_err(db_error)? {
            let event = TaskEvent {
                sequence: row.get(0).map_err(db_error)?,
                event: serde_json::from_str(&row.get::<_, String>(1).map_err(db_error)?)
                    .map_err(db_error)?,
            };
            let size = serde_json::to_vec(&event).map_err(db_error)?.len() + 1;
            if events.len() == 100 || bytes + size > 1_048_576 {
                if events.is_empty() {
                    return Err(
                        "native event exceeds the 1 MiB page budget; inspect the host journal"
                            .into(),
                    );
                }
                has_more = true;
                break;
            }
            bytes += size;
            events.push(event);
        }
        let next_cursor = events.last().map(|e| e.sequence).unwrap_or(after);
        let approval_receipts={
            let mut stmt=tx.prepare("SELECT request_id,decision,state,original_request FROM task_responses WHERE task_id=?1 ORDER BY rowid").map_err(db_error)?;
            let rows=stmt.query_map([id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?))).map_err(db_error)?;
            rows.map(|row|{let (request_id,decision,state,binding)=row.map_err(db_error)?; Ok(super::ApprovalReceipt{request_id,decision:serde_json::from_str(&decision).map_err(db_error)?,state,original_request:binding.and_then(|b|serde_json::from_str(&b).ok())})}).collect::<Result<Vec<_>,String>>()?
        };
        Ok(TaskRead {
            approval_receipts,
            task,
            events,
            next_cursor,
            has_more,
        })
    }
    pub fn update(&self, id: &str, f: impl FnOnce(&mut TaskSnapshot)) -> Result<(), String> {
        self.append_optional(id, None, f)
    }
    pub fn claim(&self, id: &str) -> Result<std::fs::File, String> {
        validate_id(id)?;
        let lease = self
            .try_lease(id)?
            .ok_or("task worker already owns its lease")?;
        let mut conn = self.conn.lock().map_err(db_error)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let task = load(&tx, id)?;
        if task.status.is_terminal() {
            return Err("task has already settled; never replay it".into());
        }
        if tx
            .execute(
                "UPDATE tasks SET claimed=1,heartbeat=?2 WHERE id=?1 AND claimed=0",
                rusqlite::params![id, chrono::Utc::now().timestamp_millis()],
            )
            .map_err(db_error)?
            != 1
        {
            return Err("task launch was already claimed; replay is forbidden".into());
        }
        tx.commit().map_err(db_error)?;
        Ok(lease)
    }
    /// An absence receipt is causal: delayed admissions of this UUID are
    /// rejected by the same SQLite write transaction, even after restart.
    pub(super) fn cancel_or_fence(&self, id: &str) -> Result<bool,String> {
        validate_id(id)?;
        let mut conn=self.conn.lock().map_err(db_error)?;
        let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(db_error)?;
        let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM tasks WHERE id=?1)",[id],|r|r.get(0)).map_err(db_error)?;
        if !exists {
            tx.execute("INSERT OR IGNORE INTO task_cancel_fences(task_id) VALUES(?1)",[id]).map_err(db_error)?;
        } else {
            let mut task=load(&tx,id)?;
            if !task.status.is_terminal() {
                task.cancel_requested=true; task.status=TaskStatus::Stopping;
                task.activity=Some("Stop requested; awaiting provider acknowledgement".into());
                save(&tx,&mut task)?;
            }
        }
        tx.commit().map_err(db_error)?;
        Ok(exists)
    }
    pub fn cancel(&self, id: &str) -> Result<(), String> {
        self.update(id, |t| {
            t.cancel_requested = true;
            t.status = TaskStatus::Stopping;
            t.activity = Some("Stop requested; awaiting provider acknowledgement".into());
        })
    }
    pub fn append(
        &self,
        id: &str,
        event: &serde_json::Value,
        f: impl FnOnce(&mut TaskSnapshot),
    ) -> Result<(), String> {
        self.append_optional(id, Some(event), f)
    }
    fn append_optional(
        &self,
        id: &str,
        event: Option<&serde_json::Value>,
        f: impl FnOnce(&mut TaskSnapshot),
    ) -> Result<(), String> {
        validate_id(id)?;
        let mut conn = self.conn.lock().map_err(db_error)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let mut task = load(&tx, id)?;
        // Late native events remain inspectable, but cannot rewrite settled facts.
        if !task.status.is_terminal() {
            f(&mut task);
            if task.cancel_requested && !task.status.is_terminal() {
                task.status = TaskStatus::Stopping;
            }
            save(&tx, &mut task)?;
        }
        if let Some(event) = event {
            tx.execute("INSERT INTO task_events (task_id,sequence,event) VALUES (?1,(SELECT COALESCE(MAX(sequence),0)+1 FROM task_events WHERE task_id=?1),?2)",rusqlite::params![id,serde_json::to_string(event).map_err(db_error)?]).map_err(db_error)?;
        }
        tx.commit().map_err(db_error)
    }
    pub fn respond(&self, id: &str, response: &RespondRequest) -> Result<(), String> {
        validate_id(id)?;
        let mut conn = self.conn.lock().map_err(db_error)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let decision=serde_json::to_string(&response.decision).map_err(db_error)?;
        let binding=response.original_request.as_ref().filter(|r|r.request_id==response.request_id).ok_or("Original approval consent identity is required")?;
        let original=serde_json::to_string(binding).map_err(db_error)?;
        if original.len()>65536 || decision.len()>65536 || response.request_id.len()>512 {return Err("Approval consent exceeds bounded wire retention".into());}
        let prior:Option<(String,Option<String>)>=tx.query_row("SELECT decision,original_request FROM task_responses WHERE task_id=?1 AND request_id=?2",rusqlite::params![id,response.request_id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(db_error)?;
        if let Some((prior,prior_binding))=prior {
            if prior!=decision || prior_binding.as_deref()!=Some(&original) {return Err("Approval conflicts with queued/attempted immutable consent".into());}
            // This acknowledges the durable enqueue/attempt, not another callback.
            return tx.commit().map_err(db_error);
        }
        let task = load(&tx, id)?;
        if task.status.is_terminal() || task.cancel_requested || !task.pending_requests.iter().any(|r|r==binding) {
            return Err("original approval kind/payload is not currently pending on this live task".into());
        }
        let count=tx.execute("INSERT OR IGNORE INTO task_responses (task_id,request_id,decision,original_request) VALUES (?1,?2,?3,?4)",rusqlite::params![id,response.request_id,decision,original]).map_err(db_error)?;
        if count != 1 {
            return Err("approval already has a queued or attempted response".into());
        }
        tx.commit().map_err(db_error)
    }
    /// Mark delivery as attempted *before* invoking a native callback. Neither
    /// worker restart nor a transport retry can blindly replay that callback.
    pub fn take_responses(&self, id: &str) -> Result<Vec<RespondRequest>, String> {
        validate_id(id)?;
        let mut conn = self.conn.lock().map_err(db_error)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let task = load(&tx, id)?;
        if task.cancel_requested || task.status.is_terminal() {
            return Ok(vec![]);
        }
        let responses = {
            let mut stmt=tx.prepare("SELECT request_id,decision,original_request FROM task_responses WHERE task_id=?1 AND state='queued' ORDER BY rowid").map_err(db_error)?;
            let mut rows = stmt.query([id]).map_err(db_error)?;
            let mut responses = vec![];
            while let Some(row) = rows.next().map_err(db_error)? {
                let request_id: String = row.get(0).map_err(db_error)?;
                let original:Option<super::RemoteApproval>=row.get::<_,Option<String>>(2).map_err(db_error)?.and_then(|b|serde_json::from_str(&b).ok());
                if original.as_ref().is_some_and(|b|b.request_id==request_id && task.pending_requests.iter().any(|r|r==b)) {
                    responses.push(RespondRequest {
                        original_request: original,
                        request_id,
                        decision: serde_json::from_str(&row.get::<_, String>(1).map_err(db_error)?)
                            .map_err(db_error)?,
                    });
                    break; // Claim one callback, never a cancellation-blind batch.
                }
            }
            responses
        };
        if let Some(response) = responses.first() {
            tx.execute(
                "UPDATE task_responses SET state='attempted' WHERE task_id=?1 AND request_id=?2 AND state='queued'",
                rusqlite::params![id, response.request_id],
            ).map_err(db_error)?;
        }
        tx.commit().map_err(db_error)?;
        Ok(responses)
    }
    pub(super) fn finish_response_attempt(&self,id:&str,request:&str,delivered:bool)->Result<(),String> {
        self.conn.lock().map_err(db_error)?.execute("UPDATE task_responses SET state=?3 WHERE task_id=?1 AND request_id=?2 AND state='attempted'",rusqlite::params![id,request,if delivered {"delivered"} else {"unknown"}]).map_err(db_error)?;
        Ok(())
    }
    pub fn reconcile(&self, id: &str) -> Result<(), String> {
        validate_id(id)?;
        // A lifetime flock, not kill(pid,0), prevents PID reuse and reboot
        // from falsely claiming that a vanished worker is still executing.
        let Some(_lease) = self.try_lease(id)? else {
            return Ok(());
        };
        let mut conn = self.conn.lock().map_err(db_error)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let mut task = load(&tx, id)?;
        let (claimed, admitted): (bool, i64) = tx
            .query_row(
                "SELECT claimed,admitted_at FROM tasks WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(db_error)?;
        if !task.status.is_terminal()
            && (claimed || chrono::Utc::now().timestamp_millis() - admitted > 10_000)
        {
            task.status = TaskStatus::Interrupted;
            task.error=Some("Worker is absent or launch acknowledgement was lost. Remote execution is uncertain; this task will not be replayed.".into());
            task.activity = None;
            task.pending_requests.clear();
            save(&tx, &mut task)?;
        }
        tx.commit().map_err(db_error)
    }
    fn try_lease(&self, id: &str) -> Result<Option<std::fs::File>, String> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(self.root.join(format!("{id}.lock")))
            .map_err(db_error)?;
        // SAFETY: the owned file descriptor is valid; lock does not access memory.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(Some(file));
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            Ok(None)
        } else {
            Err(db_error(error))
        }
    }
    pub fn open(root: &Path) -> Result<Self, String> {
        let root = root.join("tasks");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&root)
            .map_err(db_error)?;
        let path = root.join("journal.sqlite3");
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(&path)
            .map_err(db_error)?;
        let conn = Connection::open(&path).map_err(db_error)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(db_error)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY, request TEXT NOT NULL, snapshot TEXT NOT NULL,
                claimed INTEGER NOT NULL DEFAULT 0, admitted_at INTEGER NOT NULL,
                heartbeat INTEGER NOT NULL, launcher TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS task_cancel_fences (task_id TEXT PRIMARY KEY);
            CREATE TABLE IF NOT EXISTS task_checkouts (task_id TEXT PRIMARY KEY, identity TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS task_events (
                task_id TEXT NOT NULL, sequence INTEGER NOT NULL, event TEXT NOT NULL,
                PRIMARY KEY(task_id,sequence)
            );
            CREATE TABLE IF NOT EXISTS task_responses (
                task_id TEXT NOT NULL, request_id TEXT NOT NULL, decision TEXT NOT NULL,
                state TEXT NOT NULL DEFAULT 'queued', PRIMARY KEY(task_id,request_id)
            );",
        )
        .map_err(db_error)?;
        // Legacy queued rows stay unbound and cannot claim a callback.
        let bound:bool=conn.prepare("PRAGMA table_info(task_responses)").map_err(db_error)?.query_map([],|r|r.get::<_,String>(1)).map_err(db_error)?.collect::<Result<Vec<_>,_>>().map_err(db_error)?.iter().any(|name|name=="original_request");
        if !bound {conn.execute_batch("ALTER TABLE task_responses ADD COLUMN original_request TEXT").map_err(db_error)?;}
        Ok(Self {
            conn: Mutex::new(conn),
            root: root.to_path_buf(),
        })
    }
    /// Returns true only for the durable first admission. Retrying does not
    /// revalidate runtime availability, spawn, or recover an uncertain launch.
    #[cfg(test)]
    pub fn admit(&self, request: &LaunchRequest) -> Result<bool, String> {
        self.admit_bound(request, None)
    }
    pub(super) fn state_root(&self) -> &Path { self.root.parent().expect("tasks root") }
    pub(super) fn checkout(&self, id: &str) -> Result<Option<super::checkout::CheckoutIdentity>, String> {
        let conn=self.conn.lock().map_err(db_error)?;
        let raw: Option<String>=conn.query_row("SELECT identity FROM task_checkouts WHERE task_id=?1",[id],|r|r.get(0)).optional().map_err(db_error)?;
        raw.map(|s|serde_json::from_str(&s).map_err(db_error)).transpose()
    }
    pub(super) fn admit_bound(&self, request: &LaunchRequest, identity: Option<&super::checkout::CheckoutIdentity>) -> Result<bool, String> {
        validate_id(&request.id)?;
        let encoded = serde_json::to_string(request).map_err(db_error)?;
        let mut conn = self.conn.lock().map_err(db_error)?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db_error)?;
        let fenced:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM task_cancel_fences WHERE task_id=?1)",[&request.id],|r|r.get(0)).map_err(db_error)?;
        if fenced {return Err("Task UUID was durably cancelled before admission; never launch it".into());}
        if let Some(existing) = tx
            .query_row(
                "SELECT request FROM tasks WHERE id=?1",
                [&request.id],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(db_error)?
        {
            if existing != encoded {
                return Err("task id conflicts with a different launch request".into());
            }
            return Ok(false);
        }
        let now = chrono::Utc::now().to_rfc3339();
        let snapshot = TaskSnapshot {
            id: request.id.clone(),
            request: request.clone(),
            child_thread_id: uuid::Uuid::new_v4().to_string(),
            provider_session_id: None,
            turn_id: None,
            status: TaskStatus::Starting,
            activity: Some("Launch durably admitted".into()),
            result: None,
            error: None,
            created_at: now.clone(),
            updated_at: now,
            cancel_requested: false,
            pending_requests: vec![],
        };
        let clock = chrono::Utc::now().timestamp_millis();
        tx.execute("INSERT INTO tasks (id,request,snapshot,admitted_at,heartbeat,launcher) VALUES (?1,?2,?3,?4,?4,?5)",
            rusqlite::params![request.id,encoded,serde_json::to_string(&snapshot).map_err(db_error)?,clock,process_identity()])
            .map_err(db_error)?;
        if let Some(identity)=identity {
            tx.execute("INSERT INTO task_checkouts(task_id,identity) VALUES(?1,?2)",rusqlite::params![request.id,serde_json::to_string(identity).map_err(db_error)?]).map_err(db_error)?;
        }
        tx.commit().map_err(db_error)?;
        Ok(true)
    }
}
fn db_error(e: impl std::fmt::Display) -> String {
    format!("task journal: {e}")
}

pub(super) fn validate_id(id: &str) -> Result<(), String> {
    if uuid::Uuid::parse_str(id)
        .map(|u| u.to_string() == id)
        .unwrap_or(false)
    {
        Ok(())
    } else {
        Err("task id must be a canonical lowercase hyphenated UUID".into())
    }
}
fn load(conn: &Connection, id: &str) -> Result<TaskSnapshot, String> {
    let raw = conn
        .query_row("SELECT snapshot FROM tasks WHERE id=?1", [id], |r| {
            r.get::<_, String>(0)
        })
        .optional()
        .map_err(db_error)?
        .ok_or_else(|| format!("unknown task id: {id}"))?;
    serde_json::from_str(&raw).map_err(db_error)
}
fn save(conn: &Connection, task: &mut TaskSnapshot) -> Result<(), String> {
    task.updated_at = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE tasks SET snapshot=?2,heartbeat=?3 WHERE id=?1",
        rusqlite::params![
            task.id,
            serde_json::to_string(task).map_err(db_error)?,
            chrono::Utc::now().timestamp_millis()
        ],
    )
    .map_err(db_error)?;
    Ok(())
}

fn process_identity() -> String {
    let pid = std::process::id();
    #[cfg(target_os = "linux")]
    {
        format!(
            "{pid}:{}",
            std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|s| s.rsplit_once(") ").map(|(_, rest)| rest
                    .split_whitespace()
                    .nth(19)
                    .unwrap_or("")
                    .to_owned()))
                .unwrap_or_default()
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        pid.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::super::RemoteApproval;
    use super::*;
    use crate::agent_provider::ApprovalDecision;
    #[test]
    fn abandoned_launch_is_interrupted_and_malformed_ids_never_form_paths() {
        let dir = tempfile::TempDir::new().unwrap();
        let request = super::super::tests::request();
        let journal = TaskStore::open(dir.path()).unwrap();
        journal.admit(&request).unwrap();
        journal
            .conn
            .lock()
            .unwrap()
            .execute("UPDATE tasks SET admitted_at=0 WHERE id=?1", [&request.id])
            .unwrap();
        journal.reconcile(&request.id).unwrap();
        assert_eq!(
            journal.snapshot(&request.id).unwrap().status,
            TaskStatus::Interrupted
        );
        assert!(!journal.admit(&request).unwrap());
        assert!(journal.claim(&request.id).is_err());
        for id in ["../escape", "00000000000000000000000000000000", ""] {
            assert!(journal.claim(id).is_err());
            assert!(journal.cancel(id).is_err());
            assert!(journal.reconcile(id).is_err());
        }
    }
    #[test]
    fn cancel_is_durable_but_not_cancelled_until_worker_ack() {
        let dir = tempfile::TempDir::new().unwrap();
        let request = super::super::tests::request();
        let journal = TaskStore::open(dir.path()).unwrap();
        journal.admit(&request).unwrap();
        assert!(
            journal.cancel(&request.id).is_ok(),
            "stop intent must be persisted"
        );
        let task = TaskStore::open(dir.path())
            .unwrap()
            .snapshot(&request.id)
            .unwrap();
        assert!(task.cancel_requested);
        assert_eq!(task.status, TaskStatus::Stopping);
        assert_ne!(task.status, TaskStatus::Cancelled);
        assert!(journal.cancel(&uuid::Uuid::new_v4().to_string()).is_err());
        journal
            .update(&request.id, |t| t.status = TaskStatus::Cancelled)
            .unwrap();
        journal
            .update(&request.id, |t| t.status = TaskStatus::Running)
            .unwrap();
        assert_eq!(
            journal.snapshot(&request.id).unwrap().status,
            TaskStatus::Cancelled,
            "terminal state immutable"
        );
    }
    #[test]
    fn worker_claim_is_once_and_dead_worker_becomes_uncertain_without_replay() {
        let dir = tempfile::TempDir::new().unwrap();
        let request = super::super::tests::request();
        let journal = TaskStore::open(dir.path()).unwrap();
        journal.admit(&request).unwrap();
        let claim = journal.claim(&request.id);
        assert!(
            claim.is_ok(),
            "one admitted worker must acquire a lifetime lease"
        );
        let lease = claim.unwrap();
        assert!(journal.claim(&request.id).is_err());
        journal.reconcile(&request.id).unwrap();
        assert_eq!(
            journal.snapshot(&request.id).unwrap().status,
            TaskStatus::Starting
        );
        drop(lease);
        journal.reconcile(&request.id).unwrap();
        assert_eq!(
            journal.snapshot(&request.id).unwrap().status,
            TaskStatus::Interrupted
        );
        assert!(journal.claim(&request.id).is_err());
        assert!(!journal.admit(&request).unwrap());
    }
    #[test]
    fn native_event_pages_have_stable_cursors_and_honest_more() {
        let dir = tempfile::TempDir::new().unwrap();
        let request = super::super::tests::request();
        let journal = TaskStore::open(dir.path()).unwrap();
        journal.admit(&request).unwrap();
        let event = serde_json::json!({"type":"runtime_warning","message":"fixture"});
        assert!(
            journal.append(&request.id, &event, |_| {}).is_ok(),
            "native events need durable ordered append"
        );
        for _ in 0..204 {
            journal.append(&request.id, &event, |_| {}).unwrap();
        }
        let a = journal.read(&request.id, 0).unwrap();
        assert_eq!(a.events.len(), 100);
        assert_eq!(a.next_cursor, 100);
        assert!(a.has_more);
        let b = journal.read(&request.id, a.next_cursor).unwrap();
        assert_eq!(b.events[0].sequence, 101);
        assert!(b.has_more);
        let c = journal.read(&request.id, b.next_cursor).unwrap();
        assert_eq!(c.events.len(), 5);
        assert_eq!(c.next_cursor, 205);
        assert!(!c.has_more);
        let tail = journal.read(&request.id, 205).unwrap();
        assert!(tail.events.is_empty());
        assert_eq!(tail.next_cursor, 205);
        assert!(journal.read(&request.id, -1).is_err());
        let large = serde_json::json!({"type":"runtime_warning","message":"x".repeat(100_000)});
        for _ in 0..20 {
            journal.append(&request.id, &large, |_| {}).unwrap();
        }
        let page = journal.read(&request.id, 205).unwrap();
        assert!(page.events.len() < 20);
        assert!(page.has_more);
        assert!(serde_json::to_vec(&page.events).unwrap().len() <= 1_048_576);
    }
    #[test]
    fn consent_boundary_receiver_binds_enqueue_and_once_only_claim() {
        for change in ["unchanged","before-enqueue","before-take","kind","legacy"] {
            let dir=tempfile::TempDir::new().unwrap();let launch=super::super::tests::request();let store=TaskStore::open(dir.path()).unwrap();store.admit(&launch).unwrap();
            let request=RemoteApproval {request_id:"opaque/01".into(),request_kind:"command".into(),payload:serde_json::json!({"cmd":"original","extra":[null,false]})};
            store.update(&launch.id,|t|{t.status=TaskStatus::AwaitingApproval;t.pending_requests=vec![request.clone()];}).unwrap();
            let parsed=serde_json::from_value::<RespondRequest>(serde_json::json!({"request_id":request.request_id,"original_request":request,"decision":{"decision":"allow"}}));
            assert!(parsed.is_ok(),"receiver wire cannot carry original full consent identity: {parsed:?}");let response=parsed.unwrap();
            if change=="before-enqueue" {store.update(&launch.id,|t|t.pending_requests[0].payload["cmd"]=serde_json::json!("changed")).unwrap();assert!(store.respond(&launch.id,&response).is_err());continue;}
            store.respond(&launch.id,&response).unwrap();
            if change=="before-take" {store.update(&launch.id,|t|t.pending_requests[0].payload["extra"][1]=serde_json::json!(true)).unwrap();}
            if change=="kind" {store.update(&launch.id,|t|t.pending_requests[0].request_kind="replacement".into()).unwrap();}
            if change=="legacy" {store.conn.lock().unwrap().execute_batch("ALTER TABLE task_responses RENAME TO bound_responses; CREATE TABLE task_responses(task_id TEXT NOT NULL,request_id TEXT NOT NULL,decision TEXT NOT NULL,state TEXT NOT NULL DEFAULT 'queued',PRIMARY KEY(task_id,request_id)); INSERT INTO task_responses SELECT task_id,request_id,decision,state FROM bound_responses; DROP TABLE bound_responses;").unwrap();}
            drop(store);let reopened=TaskStore::open(dir.path()).unwrap();
            assert_eq!(reopened.take_responses(&launch.id).unwrap().len(),usize::from(change=="unchanged"),"receiver callback claim accepted changed/unproven binding: {change}");
            assert!(reopened.take_responses(&launch.id).unwrap().is_empty());
        }
    }
    #[test]
    fn approvals_only_queue_live_pending_requests_and_never_replay_delivery() {
        let dir = tempfile::TempDir::new().unwrap();
        let request = super::super::tests::request();
        let journal = TaskStore::open(dir.path()).unwrap();
        journal.admit(&request).unwrap();
        let response = RespondRequest {
            original_request:Some(RemoteApproval {request_id:"approval".into(),request_kind:"tool".into(),payload:serde_json::json!({})}),
            request_id: "approval".into(),
            decision: ApprovalDecision::Deny {
                message: "No".into(),
            },
        };
        assert!(journal.respond(&request.id, &response).is_err());
        let update = journal.update(&request.id, |t| {
            t.status = TaskStatus::AwaitingApproval;
            t.pending_requests.push(RemoteApproval {
                request_id: "approval".into(),
                request_kind: "tool".into(),
                payload: serde_json::json!({}),
            });
        });
        assert!(update.is_ok(), "native approval snapshots must persist");
        journal.respond(&request.id, &response).unwrap();
        assert!(journal.respond(&request.id, &response).is_ok());
        assert_eq!(journal.take_responses(&request.id).unwrap().len(), 1);
        assert!(journal.take_responses(&request.id).unwrap().is_empty());
        journal.cancel(&request.id).unwrap();
        assert!(journal.respond(&request.id, &response).is_ok());
        let conflict=RespondRequest{original_request:response.original_request.clone(),request_id:response.request_id.clone(),decision:ApprovalDecision::Allow{updated_input:None,updated_permissions:None}};
        assert!(journal.respond(&request.id,&conflict).is_err());
    }
    #[test]
    fn launch_intent_survives_reopen_and_exact_retry_never_readmits() {
        let dir = tempfile::TempDir::new().unwrap();
        let request = super::super::tests::request();
        let journal = TaskStore::open(dir.path()).unwrap();
        assert!(
            journal.admit(&request).unwrap(),
            "first launch must durably admit before spawn"
        );
        drop(journal);
        let journal = TaskStore::open(dir.path()).unwrap();
        assert!(
            !journal.admit(&request).unwrap(),
            "retry must not execute again"
        );
        let mut conflicting = request;
        conflicting.prompt.push_str(" different");
        assert!(
            journal.admit(&conflicting).is_err(),
            "conflicting reuse must fail"
        );
    }
}
