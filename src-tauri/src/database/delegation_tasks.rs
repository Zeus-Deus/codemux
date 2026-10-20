//! Canonical task identity, immutable results, and completion ownership.
//! No session FK: task lineage/results must survive closing or deleting a pane.
use super::{message_delivery, DatabaseStore};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

fn same_admission_payload(saved: &str, requested: &Value) -> bool {
    let Ok(mut saved) = serde_json::from_str::<Value>(saved) else { return false; };
    let mut requested = requested.clone();
    // Waiting changes only the receipt, never the admitted work. Strip these
    // controls on both sides so pre-upgrade payloads remain retryable.
    for value in [&mut saved, &mut requested] {
        if let Some(object) = value.as_object_mut() { object.remove("mode"); object.remove("wait_ms"); }
    }
    saved == requested
}

pub(super) fn create_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS agent_delegation_tasks (
        task_id TEXT PRIMARY KEY, parent_thread_id TEXT NOT NULL,
        child_thread_id TEXT, backend TEXT NOT NULL, client_request_id TEXT,
        admission_payload TEXT NOT NULL, record TEXT NOT NULL,
        UNIQUE(parent_thread_id, client_request_id));
        CREATE INDEX IF NOT EXISTS delegation_task_child ON agent_delegation_tasks(child_thread_id);
        CREATE INDEX IF NOT EXISTS delegation_task_parent ON agent_delegation_tasks(parent_thread_id);
        CREATE TABLE IF NOT EXISTS agent_delegation_read_coverage (
            parent_thread_id TEXT NOT NULL, task_id TEXT NOT NULL, start_offset INTEGER NOT NULL, end_offset INTEGER NOT NULL,
            PRIMARY KEY(parent_thread_id,task_id,start_offset,end_offset));
        CREATE TABLE IF NOT EXISTS agent_orchestration_context (
            thread_id TEXT PRIMARY KEY, root_id TEXT NOT NULL, depth INTEGER NOT NULL,
            hop INTEGER NOT NULL, causation_id TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS agent_orchestration_admissions (
            id TEXT PRIMARY KEY, root_id TEXT NOT NULL, kind TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS orchestration_admission_root ON agent_orchestration_admissions(root_id);
        CREATE INDEX IF NOT EXISTS delegation_task_budget ON agent_delegation_tasks(json_extract(record,'$.budget_root_id'),json_extract(record,'$.status'),json_extract(record,'$.workState'));
        CREATE TABLE IF NOT EXISTS agent_orchestration_launches (
            id TEXT PRIMARY KEY, thread_id TEXT UNIQUE NOT NULL, root_id TEXT NOT NULL,
            executing INTEGER NOT NULL DEFAULT 1);
        CREATE INDEX IF NOT EXISTS orchestration_launch_root ON agent_orchestration_launches(root_id,executing);
        CREATE TABLE IF NOT EXISTS agent_message_causation (
            id TEXT PRIMARY KEY, sender TEXT NOT NULL, target TEXT NOT NULL, root_id TEXT NOT NULL,
            hop INTEGER NOT NULL, causation_id TEXT NOT NULL, fingerprint TEXT NOT NULL,
            UNIQUE(sender,target,causation_id,fingerprint));
        CREATE TABLE IF NOT EXISTS agent_message_active_causation (
            thread_id TEXT PRIMARY KEY, root_id TEXT NOT NULL, hop INTEGER NOT NULL, causation_id TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS agent_delegation_imports (
            source_id TEXT NOT NULL, source_version INTEGER NOT NULL, task_id TEXT NOT NULL,
            PRIMARY KEY(source_id, source_version));")
        .map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskBackend {
    Local,
    Ssh,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Running,
    Waiting,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}
impl TaskStatus {
    pub fn terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkState {
    Working,
    WaitingForChildren,
    ResultAvailable,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingRequest {
    pub id: String,
    pub kind: String,
    pub created_at: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublishedResult {
    pub version: u64,
    pub run_id: Option<String>,
    pub text: String,
    pub status: TaskStatus,
    pub published_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskRecord {
    pub task_id: String,
    #[serde(default)]
    pub caller: Option<crate::agent_control::ControlCaller>,
    #[serde(default)]
    pub budget_root_id: Option<String>,
    pub backend: TaskBackend,
    pub parent_thread_id: String,
    pub parent_run_id: Option<String>,
    #[serde(default)]
    pub parent_title: Option<String>,
    pub root_task_id: String,
    pub lineage: Vec<String>,
    pub child_thread_id: Option<String>,
    pub original_child_run_id: Option<String>,
    pub child_run_ids: Vec<String>,
    pub latest_terminal_run_id: Option<String>,
    pub has_pending_child_runs: bool,
    pub provider: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub plan_mode: bool,
    pub role: Option<String>,
    pub title: String,
    pub brief: String,
    pub workspace_id: String,
    pub cwd: String,
    pub isolation: String,
    pub worktree: Option<Value>,
    pub preparation_state: String,
    pub status: TaskStatus,
    #[serde(rename = "workState", alias = "work_state")]
    pub work_state: WorkState,
    pub progress: Option<String>,
    pub result: Option<PublishedResult>,
    /// Parent Stop generation captured before admission; absent on legacy rows.
    #[serde(default)]
    pub admission_generation: Option<u64>,
    pub cancellation_generation: u64,
    pub completion_delivery: String,
    pub delivery_id: Option<String>,
    pub delivered_at: Option<i64>,
    pub pending_request: Option<PendingRequest>,
    pub tool_call_id: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub host: Option<Value>,
    pub remote_task_id: Option<String>,
    #[serde(default)]
    pub wait_until: Option<i64>,
}
impl TaskRecord {
    pub fn new(
        task_id: String,
        parent: String,
        child: Option<String>,
        provider: String,
        title: String,
        brief: String,
        now: i64,
    ) -> Self {
        Self {
            caller: None,
            budget_root_id: None,
            root_task_id: task_id.clone(),
            task_id,
            backend: TaskBackend::Local,
            parent_thread_id: parent,
            parent_run_id: None,
            parent_title: None,
            lineage: vec![],
            child_thread_id: child,
            original_child_run_id: None,
            child_run_ids: vec![],
            latest_terminal_run_id: None,
            has_pending_child_runs: false,
            provider,
            model: None,
            effort: None,
            permission_mode: None,
            plan_mode: false,
            role: None,
            title,
            brief,
            workspace_id: String::new(),
            cwd: String::new(),
            isolation: "tab".into(),
            worktree: None,
            preparation_state: "admitted".into(),
            status: TaskStatus::Queued,
            work_state: WorkState::Working,
            progress: None,
            result: None,
            admission_generation: None,
            cancellation_generation: 0,
            completion_delivery: "pending".into(),
            delivery_id: None,
            delivered_at: None,
            pending_request: None,
            tool_call_id: None,
            created_at: now,
            started_at: None,
            completed_at: None,
            host: None,
            remote_task_id: None,
            wait_until: None,
        }
    }
}
fn decode(data: String) -> Result<TaskRecord, String> {
    serde_json::from_str(&data).map_err(|e| e.to_string())
}
fn read(conn: &Connection, id: &str) -> Result<Option<TaskRecord>, String> {
    conn.query_row(
        "SELECT record FROM agent_delegation_tasks WHERE task_id=?1",
        [id],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .map_err(|e| e.to_string())?
    .map(decode)
    .transpose()
}
fn write(conn: &Connection, task: &TaskRecord) -> Result<(), String> {
    conn.execute(
        "UPDATE agent_delegation_tasks SET child_thread_id=?2,record=?3 WHERE task_id=?1",
        params![
            task.task_id,
            task.child_thread_id,
            serde_json::to_string(task).map_err(|e| e.to_string())?
        ],
    )
    .map(drop)
    .map_err(|e| e.to_string())
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct RootContext {
    pub root_id: String,
    pub depth: usize,
    pub hop: u32,
    pub causation_id: String,
}
/// The initial task message spends the task's existing reservation. It must
/// preserve the child's lineage context rather than treating it as a new send.
pub(super) fn admitted_task_start(
    tx: &Connection,
    sender: &str,
    target: &str,
    payload: &Value,
) -> Result<bool, String> {
    if payload["origin"] != "delegation" || payload["caller"]["admitted"]["kind"] != "task" {
        return Ok(false);
    }
    let Some(id) = payload["caller"]["admitted"]["task_id"].as_str() else {
        return Ok(false);
    };
    Ok(read(tx, id)?.is_some_and(|task| {
        task.parent_thread_id == sender && task.child_thread_id.as_deref() == Some(target)
    }))
}
pub(super) fn admit_agent_message(tx: &Connection, sender: &str, target: &str, id: &str, payload: &Value) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let context = message_context(tx, sender)?;
    if context.hop >= 24 {
        return Err("resource_limit: automatic agent message chain reached 24 hops; a human message is required to start a new chain".into());
    }
    let fingerprint = format!("{:x}", Sha256::digest(payload["input"]["text"].as_str().unwrap_or_default().as_bytes()));
    let inserted = tx.execute("INSERT OR IGNORE INTO agent_message_causation(id,sender,target,root_id,hop,causation_id,fingerprint) VALUES (?1,?2,?3,?4,?5,?6,?7)",params![id,sender,target,context.root_id,context.hop+1,context.causation_id,fingerprint]).map_err(|e|e.to_string())?;
    if inserted == 0 { return Err("duplicate_causation: identical agent message already admitted for this cause".into()); }
    // Messages carry causation separately. They never replace a recipient's
    // human work root or reserve a new-work admission.
    Ok(())
}
fn root_context(conn: &Connection, thread: &str) -> Result<RootContext, String> {
    conn.execute("INSERT OR IGNORE INTO agent_orchestration_context(thread_id,root_id,depth,hop,causation_id) VALUES (?1,?2,0,0,?2)",params![thread,uuid::Uuid::new_v4().to_string()]).map_err(|e|e.to_string())?;
    conn.query_row("SELECT root_id,depth,hop,causation_id FROM agent_orchestration_context WHERE thread_id=?1",[thread],|r|Ok(RootContext{root_id:r.get(0)?,depth:r.get(1)?,hop:r.get(2)?,causation_id:r.get(3)?})).map_err(|e|e.to_string())
}
fn message_context(conn: &Connection, thread: &str) -> Result<RootContext, String> {
    conn.query_row("SELECT root_id,hop,causation_id FROM agent_message_active_causation WHERE thread_id=?1",[thread],|r|Ok(RootContext{root_id:r.get(0)?,depth:0,hop:r.get(1)?,causation_id:r.get(2)?})).optional().map_err(|e|e.to_string())?.map(Ok).unwrap_or_else(||root_context(conn,thread))
}
pub(super) fn activate_message_causation(conn: &Connection, id: &str) -> Result<(), String> {
    conn.execute("INSERT INTO agent_message_active_causation(thread_id,root_id,hop,causation_id) SELECT target,root_id,hop,id FROM agent_message_causation WHERE id=?1 ON CONFLICT(thread_id) DO UPDATE SET root_id=excluded.root_id,hop=excluded.hop,causation_id=excluded.causation_id",[id]).map(drop).map_err(|e|e.to_string())
}
fn waiting_on_children(conn: &Connection, parent: &str) -> Result<bool, String> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM agent_delegation_tasks WHERE parent_thread_id=?1 AND json_extract(record,'$.wait_until')>?2)",params![parent,chrono::Utc::now().timestamp_millis()],|r|r.get(0)).map_err(|e|e.to_string())
}
fn refresh_waiting(conn: &Connection, parent: &str) -> Result<(), String> {
    let waiting = waiting_on_children(conn, parent)?;
    let data:Option<String> = conn.query_row("SELECT record FROM agent_delegation_tasks WHERE child_thread_id=?1 AND backend='local' ORDER BY rowid DESC LIMIT 1",[parent],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
    if let Some(mut task) = data.map(decode).transpose()? {
        if !task.status.terminal() {
            task.work_state = if waiting {WorkState::WaitingForChildren} else {WorkState::Working};
            write(conn, &task)?;
        }
    }
    Ok(())
}
fn active_work(conn: &Connection, root: &str, yielding: Option<&str>) -> Result<Vec<String>, String> {
    // Both halves are root-indexed. Message follow-ups have no new-work
    // reservation and therefore do not consume execution slots.
    let mut statement = conn.prepare("SELECT task_id FROM agent_delegation_tasks t
        WHERE json_extract(record,'$.budget_root_id')=?1
        AND task_id IN (SELECT id FROM agent_orchestration_admissions WHERE root_id=?1 AND kind='task')
        AND json_extract(record,'$.status') IN ('queued','running','waiting')
        AND (json_extract(record,'$.workState')<>'waiting_for_children'
            OR (EXISTS(SELECT 1 FROM agent_delegation_tasks c WHERE c.parent_thread_id=t.child_thread_id AND json_extract(c.record,'$.wait_until') IS NOT NULL)
                AND NOT EXISTS(SELECT 1 FROM agent_delegation_tasks c WHERE c.parent_thread_id=t.child_thread_id AND json_extract(c.record,'$.wait_until')>?3)))
        AND (?2 IS NULL OR child_thread_id IS NULL OR child_thread_id<>?2)
        UNION ALL SELECT id FROM agent_orchestration_launches l
        WHERE root_id=?1 AND executing=1 AND (?2 IS NULL OR thread_id<>?2)
        AND NOT EXISTS(SELECT 1 FROM agent_delegation_tasks c WHERE c.parent_thread_id=l.thread_id AND json_extract(c.record,'$.wait_until')>?3)").map_err(|e|e.to_string())?;
    let rows = statement.query_map(params![root,yielding,chrono::Utc::now().timestamp_millis()],|r|r.get(0)).map_err(|e|e.to_string())?.collect::<Result<Vec<String>,_>>().map_err(|e|e.to_string())?;
    Ok(rows)
}
fn reserve_work(conn: &Connection, root: &str, depth: usize, id: &str, kind: &str, yielding: Option<&str>) -> Result<(), String> {
    let active = active_work(conn,root,yielding)?;
    if depth > 4 || active.len() >= 3 { return Err(format!("resource_limit: depth 4, 3 executing per root; active work IDs: {}",active.join(", "))); }
    let count:u32 = conn.query_row("SELECT COUNT(*) FROM agent_orchestration_admissions WHERE root_id=?1 AND kind IN ('task','launch')",[root],|r|r.get(0)).map_err(|e|e.to_string())?;
    if count >= 6 { return Err("resource_limit: 6 new-work admissions per human root".into()); }
    conn.execute("INSERT INTO agent_orchestration_admissions(id,root_id,kind) VALUES (?1,?2,?3)",params![id,root,kind]).map(drop).map_err(|e|e.to_string())
}
pub(super) fn admit_launch(conn: &Connection, sender: &str, target: &str, id: &str) -> Result<(), String> {
    let context = root_context(conn,sender)?;
    reserve_work(conn,&context.root_id,context.depth+1,id,"launch",None)?;
    conn.execute("INSERT INTO agent_orchestration_launches(id,thread_id,root_id) VALUES (?1,?2,?3)",params![id,target,context.root_id]).map_err(|e|e.to_string())?;
    conn.execute("INSERT INTO agent_orchestration_context(thread_id,root_id,depth,hop,causation_id) VALUES (?1,?2,?3,0,?4)",params![target,context.root_id,context.depth+1,id]).map(drop).map_err(|e|e.to_string())
}
/// A direct-owner follow-up has a fresh result identity in the existing child.
/// This is part of the message transaction, so retries cannot create tasks twice.
pub(super) fn admit_followup(
    tx: &Connection,
    parent: &str,
    child: &str,
    delivery: &str,
    payload: &Value,
) -> Result<(), String> {
    let prior:Option<String>=tx.query_row("SELECT record FROM agent_delegation_tasks WHERE parent_thread_id=?1 AND child_thread_id=?2 AND backend='local' ORDER BY rowid DESC LIMIT 1",params![parent,child],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
    let Some(prior) = prior
        .map(decode)
        .transpose()?
        .filter(|t| t.result.is_some())
    else {
        return Ok(());
    };
    let id = format!("local:followup:{delivery}");
    let mut task = TaskRecord::new(
        id.clone(),
        parent.into(),
        Some(child.into()),
        prior.provider,
        prior.title,
        payload["input"]["text"].as_str().unwrap_or_default().into(),
        chrono::Utc::now().timestamp_millis(),
    );
    let binding = super::agent_chat_session(tx, child).ok_or("Follow-up target missing")?;
    task.provider = binding.provider;
    task.workspace_id = binding.workspace_id;
    task.cwd = binding.cwd.unwrap_or(prior.cwd);
    task.model = binding.model;
    task.effort = binding.effort;
    task.permission_mode = binding.permission_mode;
    task.isolation = prior.isolation;
    task.worktree = prior.worktree;
    let root: String = tx
        .query_row(
            "SELECT root_id FROM agent_orchestration_context WHERE thread_id=?1",
            [parent],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    task.budget_root_id = Some(root.clone());
    task.status = TaskStatus::Queued;
    task.has_pending_child_runs = true;
    let ancestor:Option<String>=tx.query_row("SELECT record FROM agent_delegation_tasks WHERE child_thread_id=?1 AND backend='local' ORDER BY rowid DESC LIMIT 1",[parent],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
    if let Some(ancestor) = ancestor.map(decode).transpose()? {
        task.lineage = ancestor.lineage;
        task.lineage.push(ancestor.task_id);
        task.root_task_id = ancestor.root_task_id;
    }
    tx.execute("INSERT INTO agent_delegation_tasks(task_id,parent_thread_id,child_thread_id,backend,admission_payload,record) VALUES (?1,?2,?3,'local',?4,?5)",params![id,parent,child,payload.to_string(),serde_json::to_string(&task).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
    Ok(())
}
impl DatabaseStore {
    pub(crate) fn delegation_read_coverage(
        &self,
        parent: &str,
        id: &str,
        start: usize,
        end: usize,
        total: usize,
    ) -> Result<bool, String> {
        let conn = self.conn.lock().unwrap();
        let task = read(&conn, id)?.ok_or("Unknown task")?;
        if task.parent_thread_id != parent {
            return Err("Task is not owned by this parent".into());
        }
        conn.execute("INSERT OR IGNORE INTO agent_delegation_read_coverage(parent_thread_id,task_id,start_offset,end_offset) VALUES (?1,?2,?3,?4)",params![parent,id,start,end]).map_err(|e|e.to_string())?;
        let mut statement=conn.prepare("SELECT start_offset,end_offset FROM agent_delegation_read_coverage WHERE parent_thread_id=?1 AND task_id=?2 ORDER BY start_offset").map_err(|e|e.to_string())?;
        let ranges = statement
            .query_map(params![parent, id], |r| {
                Ok((r.get::<_, usize>(0)?, r.get::<_, usize>(1)?))
            })
            .map_err(|e| e.to_string())?;
        let mut covered = 0;
        for range in ranges {
            let (start, end) = range.map_err(|e| e.to_string())?;
            if start > covered {
                break;
            }
            covered = covered.max(end);
        }
        Ok(covered >= total)
    }
    pub(crate) fn orchestration_human_root(&self, thread: &str) -> Result<(), String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e|e.to_string())?;
        let id = uuid::Uuid::new_v4().to_string();
        tx.execute("DELETE FROM agent_message_active_causation WHERE thread_id=?1",[thread]).map_err(|e|e.to_string())?;
        tx.execute("INSERT INTO agent_orchestration_context(thread_id,root_id,depth,hop,causation_id) VALUES (?1,?2,0,0,?2) ON CONFLICT(thread_id) DO UPDATE SET root_id=excluded.root_id,depth=0,hop=0,causation_id=excluded.causation_id",params![thread,id]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e|e.to_string())
    }
    pub(crate) fn orchestration_context(&self, thread: &str) -> Result<RootContext, String> {
        root_context(&self.conn.lock().unwrap(),thread)
    }
    pub(crate) fn orchestration_launch_finished(&self, thread: &str) -> Result<(), String> {
        self.conn.lock().unwrap().execute("UPDATE agent_orchestration_launches SET executing=0 WHERE thread_id=?1",[thread]).map(drop).map_err(|e|e.to_string())
    }
    pub(crate) fn delegation_waiting_on_children(&self, parent: &str) -> bool {
        waiting_on_children(&self.conn.lock().unwrap(),parent).unwrap_or(false)
    }
    /// PR1 result memberships used child IDs. Reuse their acceptance evidence
    /// when importing an old open-task row into the canonical task table.
    pub(crate) fn legacy_delegation_delivery(
        &self,
        parent: &str,
        child: &str,
    ) -> Result<Option<message_delivery::DeliveryRecord>, String> {
        let id: Option<String> = self.conn.lock().unwrap().query_row("SELECT delivery_id FROM agent_message_delivery_results WHERE target_thread_id=?1 AND task_id=?2 AND result_version=1",params![parent,child],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
        id.map(|id| self.message_delivery(&id))
            .transpose()
            .map(Option::flatten)
    }
    /// Old suppression did not distinguish human activity from Stop. Prefer
    /// retaining a result when a saved interruption followed its completion;
    /// generation-only evidence is conservatively treated as Stop too.
    pub(crate) fn legacy_parent_stop_after(&self, thread: &str, finished: &str) -> Result<bool, String> {
        self.conn.lock().unwrap().query_row("SELECT EXISTS(SELECT 1 FROM agent_chat_messages
            WHERE thread_id=?1 AND datetime(created_at)>=datetime(?2)
            AND json_extract(payload,'$.type')='turn_completed'
            AND json_extract(payload,'$.status.subtype') IN ('interrupted','aborted','cancelled'))
            OR EXISTS(SELECT 1 FROM agent_thread_delivery_state WHERE thread_id=?1 AND generation>0)",
            params![thread, finished], |r| r.get(0)).map_err(|e| e.to_string())
    }
    pub fn retry_delegation_delivery(&self, id: &str) -> Result<(), String> {
        let task = self.delegation_task(id)?.ok_or("Unknown task")?;
        if matches!(
            task.completion_delivery.as_str(),
            "disposed_by_stop" | "disposed_by_close" | "acknowledged"
        ) {
            return Err("Result was disposed or acknowledged".into());
        }
        let delivery = task.delivery_id.ok_or("No admitted delivery to retry")?;
        self.retry_message_delivery(&task.parent_thread_id, &delivery)
    }
    pub fn release_delegation_wait(&self, id: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        if let Some(mut task) = read(&conn, id)? {
            task.wait_until = None;
            if let Some(delivery) = &task.delivery_id {
                conn.execute(
                    "UPDATE agent_message_deliveries SET retry_at=NULL WHERE id=?1",
                    [delivery],
                )
                .map_err(|e| e.to_string())?;
            }
            write(&conn, &task)?;
            refresh_waiting(&conn, &task.parent_thread_id)?;
        }
        Ok(())
    }
    /// A retry can wait for an async-admitted task. Concurrent waiters share
    /// the earliest deadline; observation never extends an existing lease.
    pub(crate) fn arm_delegation_wait(&self, id: &str, until: i64) -> Result<i64, String> {
        let conn = self.conn.lock().unwrap();
        let mut task = read(&conn, id)?.ok_or("Unknown task")?;
        let until = task.wait_until.map_or(until, |saved| saved.min(until));
        task.wait_until = Some(until);
        if let Some(delivery) = &task.delivery_id {
            conn.execute("UPDATE agent_message_deliveries SET retry_at=datetime(?2/1000.0,'unixepoch') WHERE id=?1 AND attempted=0 AND state='pending'",params![delivery,until]).map_err(|e|e.to_string())?;
        }
        write(&conn, &task)?;
        Ok(until)
    }
    pub fn has_restart_resume(&self, child: &str) -> bool {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM agent_chat_restart_resumes WHERE thread_id=?1)",
                [child],
                |r| r.get(0),
            )
            .unwrap_or(false)
    }
    pub fn delegation_task(&self, id: &str) -> Result<Option<TaskRecord>, String> {
        let conn = self.conn.lock().unwrap();
        let mut task = read(&conn, id)?;
        if let Some(task) = task.as_mut() {
            if let Some(id) = &task.delivery_id {
                let state: Option<(String, Option<String>)> = conn
                    .query_row(
                        "SELECT state,error FROM agent_message_deliveries WHERE id=?1",
                        [id],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()
                    .map_err(|e| e.to_string())?;
                if let Some((state, _)) = state {
                    if !matches!(
                        task.completion_delivery.as_str(),
                        "acknowledged" | "disposed_by_stop" | "disposed_by_close"
                    ) {
                        task.completion_delivery = if state == "accepted" {
                            "delivered".into()
                        } else {
                            state
                        };
                    }
                }
            }
        }
        Ok(task)
    }
    pub fn delegation_tasks(&self, parent: Option<&str>) -> Result<Vec<TaskRecord>, String> {
        let ids: Vec<String> = {
            let conn = self.conn.lock().unwrap();
            let mut s=conn.prepare("SELECT task_id FROM agent_delegation_tasks WHERE ?1 IS NULL OR parent_thread_id=?1 ORDER BY rowid").map_err(|e|e.to_string())?;
            let rows = s
                .query_map([parent], |r| r.get(0))
                .map_err(|e| e.to_string())?
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())?;
            rows
        };
        ids.iter()
            .filter_map(|id| self.delegation_task(id).transpose())
            .collect()
    }
    pub fn delegation_task_for_child(&self, child: &str) -> Result<Option<TaskRecord>, String> {
        let id:Option<String>=self.conn.lock().unwrap().query_row("SELECT task_id FROM agent_delegation_tasks WHERE child_thread_id=?1 AND backend='local' ORDER BY rowid DESC LIMIT 1",[child],|r|r.get(0)).optional().map_err(|e|e.to_string())?;
        id.map(|id| self.delegation_task(&id))
            .transpose()
            .map(Option::flatten)
    }
    pub fn delegation_retry(
        &self,
        parent: &str,
        key: &str,
        payload: &Value,
    ) -> Result<Option<TaskRecord>, String> {
        let conn = self.conn.lock().unwrap();
        let prior:Option<(String,String)>=conn.query_row("SELECT record,admission_payload FROM agent_delegation_tasks WHERE parent_thread_id=?1 AND client_request_id=?2",params![parent,key],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|e.to_string())?;
        prior
            .map(|(data, old)| {
                if !same_admission_payload(&old, payload) {
                    return Err("client_request_id_conflict: retry payload changed".into());
                }
                decode(data)
            })
            .transpose()
    }
    /// Receipt, launch intent, resolved configuration and child identity are atomic.
    pub fn admit_delegation_task(
        &self,
        task: &TaskRecord,
        key: Option<&str>,
        payload: &Value,
    ) -> Result<(TaskRecord, bool), String> {
        if task.child_thread_id.as_deref() == Some(&task.parent_thread_id)
            || task.lineage.contains(&task.task_id)
            || task
                .lineage
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != task.lineage.len()
        {
            return Err("invalid_task_lineage: tasks must be acyclic".into());
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        if let Some(prior) = read(&tx, &task.task_id)? {
            return Ok((prior, false));
        }
        if let Some(key) = key {
            let prior:Option<(String,String)>=tx.query_row("SELECT record,admission_payload FROM agent_delegation_tasks WHERE parent_thread_id=?1 AND client_request_id=?2",params![task.parent_thread_id,key],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|e.to_string())?;
            if let Some((data, old)) = prior {
                if !same_admission_payload(&old, payload) {
                    return Err("client_request_id_conflict: retry payload changed".into());
                }
                return Ok((decode(data)?, false));
            }
        }
        let generation: u64 = tx.query_row(
            "SELECT generation FROM agent_thread_delivery_state WHERE thread_id=?1",
            [&task.parent_thread_id], |r| r.get(0),
        ).optional().map_err(|e| e.to_string())?.unwrap_or(0);
        if task.admission_generation.is_some_and(|admitted| admitted != generation) {
            return Err("Parent was stopped before task admission".into());
        }
        let mut task = task.clone();
        task.admission_generation = Some(generation);
        if let Some(root) = &task.budget_root_id {
            let context = root_context(&tx,&task.parent_thread_id)?;
            let yielding = (payload["mode"] == "wait").then_some(task.parent_thread_id.as_str());
            reserve_work(&tx,root,context.depth+1,&task.task_id,"task",yielding)?;
            if let Some(child) = &task.child_thread_id {
                tx.execute("INSERT INTO agent_orchestration_context(thread_id,root_id,depth,hop,causation_id) VALUES (?1,?2,?3,0,?4)",params![child,root,context.depth+1,task.task_id]).map_err(|e|e.to_string())?;
            }
        }
        if task.backend == TaskBackend::Local {
            if let Some(child) = &task.child_thread_id {
                tx.execute("INSERT INTO agent_chat_sessions (thread_id,workspace_id,cwd,provider,title,model,effort,permission_mode) VALUES (?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(thread_id) DO NOTHING",params![child,task.workspace_id,task.cwd,task.provider,task.title,task.model,task.effort,task.permission_mode]).map_err(|e|e.to_string())?;
            }
        }
        tx.execute("INSERT INTO agent_delegation_tasks (task_id,parent_thread_id,child_thread_id,backend,client_request_id,admission_payload,record) VALUES (?1,?2,?3,?4,?5,?6,?7)",params![task.task_id,task.parent_thread_id,task.child_thread_id,if task.backend==TaskBackend::Local {"local"}else{"ssh"},key,payload.to_string(),serde_json::to_string(&task).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
        refresh_waiting(&tx, &task.parent_thread_id)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok((task.clone(), true))
    }
    pub fn update_delegation_task(
        &self,
        id: &str,
        change: impl FnOnce(&mut TaskRecord),
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let mut task = read(&conn, id)?.ok_or("Unknown task")?;
        let original = task.clone();
        let published = task.result.clone();
        change(&mut task);
        if published.is_some() {
            task.result = published;
        }
        if task == original { return Ok(()); }
        write(&conn, &task)
    }
    /// The result and its durable delivery membership commit before any UI event.
    pub fn publish_delegation_result(
        &self,
        id: &str,
        result: &PublishedResult,
        payload: &Value,
        epoch: &str,
        source: Option<(&str, i64)>,
    ) -> Result<(), String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let mut task = read(&tx, id)?.ok_or("Unknown task")?;
        if task.result.is_none() {
            task.result = Some(result.clone());
            task.status = result.status.clone();
            task.work_state = WorkState::ResultAvailable;
            task.progress = Some(
                match result.status {
                    TaskStatus::Completed => "Finished",
                    TaskStatus::Failed => "Failed",
                    TaskStatus::Cancelled => "Stopped",
                    TaskStatus::Interrupted => "Interrupted",
                    _ => "Result available",
                }
                .into(),
            );
            task.completed_at = if result.status == TaskStatus::Interrupted
                && matches!(task.preparation_state.as_str(), "interrupted_on_restart" | "not_started")
            {
                None
            } else {
                Some(result.published_at)
            };
            task.pending_request = None;
            task.has_pending_child_runs = false;
        }
        let generation: u64 = tx
            .query_row(
                "SELECT generation FROM agent_thread_delivery_state WHERE thread_id=?1",
                [&task.parent_thread_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .unwrap_or(0);
        if task.admission_generation.is_some_and(|admitted| admitted != generation) && !task.completion_delivery.starts_with("disposed_by_") {
            task.completion_delivery = "disposed_by_stop".into();
        }
        if !task.completion_delivery.starts_with("disposed_by_") && task.delivery_id.is_none()
            && !matches!(
                task.completion_delivery.as_str(),
                "acknowledged" | "held_unknown" | "delivered" | "held"
            )
        {
            let delivery = format!("task-result:{}:1", task.task_id);
            message_delivery::admit_in_transaction(
                &tx,
                &delivery,
                &task.parent_thread_id,
                &delivery,
                payload,
                epoch,
                generation,
                None,
                false,
            )?;
            if let Some(until) = task.wait_until {
                tx.execute("UPDATE agent_message_deliveries SET retry_at=datetime(?2/1000.0,'unixepoch') WHERE id=?1",params![delivery,until]).map_err(|e|e.to_string())?;
            }
            task.delivery_id = Some(delivery);
            task.completion_delivery = "pending".into();
        }
        write(&tx, &task)?;
        if let Some((source, version)) = source {
            tx.execute("INSERT OR IGNORE INTO agent_delegation_imports(source_id,source_version,task_id) VALUES (?1,?2,?3)",params![source,version,id]).map_err(|e|e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }
    /// Only a complete terminal result read by its direct owner consumes a wake.
    pub fn dispose_delegation_task(&self, parent: &str, id: &str) -> Result<(), String> {
        self.dispose_task_result(parent, id, "disposed_by_stop")
    }
    pub(crate) fn dispose_delegation_task_for_close(&self, parent: &str, id: &str) -> Result<(), String> {
        self.dispose_task_result(parent, id, "disposed_by_close")
    }
    fn dispose_task_result(&self, parent: &str, id: &str, reason: &str) -> Result<(), String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let mut task = read(&tx, id)?.ok_or("Unknown task")?;
        if task.parent_thread_id != parent {
            return Err("Task is not owned by this thread".into());
        }
        if !task.completion_delivery.starts_with("disposed_by_") {
            task.cancellation_generation += 1;
        }
        task.completion_delivery = reason.into();
        if let Some(delivery) = &task.delivery_id {
            tx.execute("UPDATE agent_message_deliveries SET state=CASE WHEN attempted=1 THEN 'held_unknown' ELSE 'cancelled' END,error=?2 WHERE id=?1 AND state<>'accepted'",params![delivery, reason]).map_err(|e|e.to_string())?;
        }
        write(&tx, &task)?;
        tx.commit().map_err(|e| e.to_string())
    }
    pub fn acknowledge_delegation_result(
        &self,
        parent: &str,
        id: &str,
    ) -> Result<TaskRecord, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let mut task = read(&tx, id)?.ok_or("Unknown task")?;
        if task.parent_thread_id != parent {
            return Err("Task is not owned by this thread".into());
        }
        if task.result.is_some() && !task.completion_delivery.starts_with("disposed_by_") {
            if let Some(delivery) = &task.delivery_id {
                tx.execute("UPDATE agent_message_deliveries SET state='cancelled',error='Terminal result read by parent' WHERE id=?1 AND attempted=0 AND state IN ('pending','preparing','queued','held')",[delivery]).map_err(|e|e.to_string())?;
            }
            task.completion_delivery = "acknowledged".into();
            write(&tx, &task)?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(task)
    }
    /// Iterative ownership traversal includes worktree placement and terminal
    /// tasks with later child runs. Independent top-level chats are untouched.
    pub fn stop_delegation_tree(&self, parent: &str) -> Result<Vec<TaskRecord>, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let mut frontier = vec![parent.to_string()];
        let mut seen = std::collections::HashSet::new();
        let mut stopped = Vec::new();
        while let Some(thread) = frontier.pop() {
            if !seen.insert(thread.clone()) {
                continue;
            }
            let rows: Vec<String> = {
                let mut s = tx
                    .prepare("SELECT record FROM agent_delegation_tasks WHERE parent_thread_id=?1")
                    .map_err(|e| e.to_string())?;
                let rows = s
                    .query_map([&thread], |r| r.get(0))
                    .map_err(|e| e.to_string())?
                    .collect::<Result<_, _>>()
                    .map_err(|e| e.to_string())?;
                rows
            };
            for row in rows {
                let mut task = decode(row)?;
                let needs_stop = !task.completion_delivery.starts_with("disposed_by_")
                    || !task.status.terminal()
                    || task.has_pending_child_runs;
                if task.backend == TaskBackend::Local {
                    if let Some(child) = &task.child_thread_id {
                        frontier.push(child.clone());
                    }
                }
                if !task.completion_delivery.starts_with("disposed_by_") {
                    task.cancellation_generation += 1;
                }
                task.completion_delivery = "disposed_by_stop".into();
                if !task.status.terminal() {
                    task.progress = Some("Stopping; provider confirmation pending".into());
                }
                if let Some(id) = &task.delivery_id {
                    tx.execute("UPDATE agent_message_deliveries SET state=CASE WHEN attempted=1 THEN 'held_unknown' ELSE 'cancelled' END,error='disposed_by_stop' WHERE id=?1 AND state<>'accepted'",[id]).map_err(|e|e.to_string())?;
                }
                write(&tx, &task)?;
                if needs_stop {
                    stopped.push(task);
                }
            }
            // The root's ordinary Stop hook already advanced its generation.
            // Descendants use the same writer in this transaction, before any
            // provider interrupt can race a new human send.
            if thread != parent { message_delivery::stop_in_transaction(&tx, &thread)?; }
            tx.execute("UPDATE pr_watches SET status='ended',generation=generation+1,stop_reason='disposed_by_stop',updated_at_ms=CAST(unixepoch('subsec')*1000 AS INTEGER) WHERE thread_id=?1 AND status='active'",[&thread]).map_err(|e|e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(stopped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Barrier, Mutex};
    fn reopen(path: &std::path::Path) -> DatabaseStore {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;PRAGMA journal_mode=WAL;")
            .unwrap();
        crate::database::create_schema(&conn).unwrap();
        DatabaseStore {
            conn: Mutex::new(conn),
        }
    }
    fn task(id: &str, child: &str) -> TaskRecord {
        let mut t = TaskRecord::new(
            id.into(),
            "parent".into(),
            Some(child.into()),
            "codex".into(),
            "Review parser".into(),
            "Inspect the parser and report all defects".into(),
            1,
        );
        t.workspace_id = "workspace".into();
        t
    }
    fn payload(id: &str, text: &str) -> Value {
        json!({"origin":"delegation","input":{"text":text},"provenance":{"notification":{"source":{"kind":"delegated_task","task_ids":[id]},"outcome":"completed"}}})
    }
    fn publish(db: &DatabaseStore, id: &str, text: &str) {
        db.publish_delegation_result(
            id,
            &PublishedResult {
                version: 1,
                run_id: Some("run-1".into()),
                text: text.into(),
                status: TaskStatus::Completed,
                published_at: 2,
            },
            &payload(id, text),
            "boot-1",
            None,
        )
        .unwrap();
    }
    #[test]
    fn delegation_delivery_legacy_parent_stop_marker_uses_completion_timestamp() {
        let db = DatabaseStore::new_in_memory();
        db.upsert_agent_chat_session("parent", "workspace", None, "codex").unwrap();
        db.append_agent_chat_message("parent", &json!({"type":"turn_completed","status":{"kind":"error","subtype":"interrupted"}}).to_string()).unwrap();
        db.conn.lock().unwrap().execute("UPDATE agent_chat_messages SET created_at='2026-10-20 15:00:00'", []).unwrap();
        assert!(db.legacy_parent_stop_after("parent", "2026-10-20T14:00:00Z").unwrap());
        assert!(!db.legacy_parent_stop_after("parent", "2026-10-20T16:00:00Z").unwrap());
    }

    #[test]
    fn delegation_delivery_stop_generation_fences_only_pre_stop_admission() {
        let db = DatabaseStore::new_in_memory();
        let mut stale = task("old", "old-child");
        stale.admission_generation = Some(db.delivery_generation("parent").unwrap().0);
        db.stop_message_deliveries("parent").unwrap();
        db.stop_delegation_tree("parent").unwrap();
        assert!(db.admit_delegation_task(&stale, None, &json!({})).unwrap_err().contains("stopped"));
        for (id, backend) in [("local", TaskBackend::Local), ("ssh", TaskBackend::Ssh)] {
            let mut fresh = task(id, &format!("{id}-child"));
            fresh.backend = backend;
            fresh.admission_generation = Some(db.delivery_generation("parent").unwrap().0);
            db.admit_delegation_task(&fresh, None, &json!({})).unwrap();
            publish(&db, id, "Fresh result after Stop");
            assert_eq!(db.delegation_task(id).unwrap().unwrap().completion_delivery, "pending");
        }
        assert!(!db.delivery_generation("parent").unwrap().1);
    }

    #[test]
    fn delegation_delivery_descendant_stop_disposes_auto_wakes_and_retains_explicit_queues() {
        let db = DatabaseStore::new_in_memory();
        db.admit_delegation_task(&task("owned", "child"), None, &json!({})).unwrap();
        for origin in ["user", "controller", "delegation", "pr_watch", "usage_resume", "auto_resume"] {
            let id = format!("{origin}-delivery");
            db.admit_message_delivery(&id, "child", &id, &json!({"origin":origin}), "boot", 0, None, false).unwrap();
            let conn = db.conn.lock().unwrap();
            conn.execute("UPDATE agent_message_deliveries SET state='queued',queued_id=?1 WHERE id=?1", [&id]).unwrap();
            conn.execute("INSERT INTO agent_chat_queued_turns(thread_id,queued_id,position,text,epoch) VALUES ('child',?1,0,'saved','boot')", [&id]).unwrap();
        }
        db.stop_message_deliveries("parent").unwrap();
        db.stop_delegation_tree("parent").unwrap();
        db.resume_message_delivery_thread("child").unwrap();
        for origin in ["user", "controller", "delegation", "pr_watch", "usage_resume", "auto_resume"] {
            let id = format!("{origin}-delivery");
            let explicit = matches!(origin, "user" | "controller");
            assert_eq!(db.message_delivery(&id).unwrap().unwrap().state, if explicit {"queued"} else {"cancelled"});
            let saved: bool = db.conn.lock().unwrap().query_row("SELECT EXISTS(SELECT 1 FROM agent_chat_queued_turns WHERE queued_id=?1)", [&id], |r| r.get(0)).unwrap();
            assert_eq!(saved, explicit, "{origin}");
        }
    }

    #[test]
    fn delegation_delivery_noop_observation_does_not_rewrite_durable_record() {
        let db = DatabaseStore::new_in_memory();
        db.admit_delegation_task(&task("a", "child"), None, &json!({})).unwrap();
        let before = db.conn.lock().unwrap().query_row("SELECT total_changes()", [], |r| r.get::<_, u64>(0)).unwrap();
        db.update_delegation_task("a", |_| {}).unwrap();
        assert_eq!(db.conn.lock().unwrap().query_row("SELECT total_changes()", [], |r| r.get::<_, u64>(0)).unwrap(), before);
        db.update_delegation_task("a", |t| t.progress = Some("Changed".into())).unwrap();
        assert_eq!(db.conn.lock().unwrap().query_row("SELECT total_changes()", [], |r| r.get::<_, u64>(0)).unwrap(), before + 1);
    }

    #[test]
    fn delegation_delivery_reopens_after_terminal_commit_and_after_wake_admission() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.sqlite");
        {
            let db = reopen(&path);
            db.admit_delegation_task(
                &task("task-a", "child-a"),
                Some("retry"),
                &json!({"task":"review"}),
            )
            .unwrap();
        }
        let db = reopen(&path);
        assert!(db
            .delegation_task("task-a")
            .unwrap()
            .unwrap()
            .result
            .is_none());
        publish(&db, "task-a", "DONE");
        drop(db);
        let db = reopen(&path);
        let t = db.delegation_task("task-a").unwrap().unwrap();
        assert_eq!(t.result.unwrap().text, "DONE");
        let rows = db.pending_message_deliveries().unwrap();
        assert_eq!(rows.len(), 1);
        let id = rows[0].id.clone();
        assert!(db.claim_message_delivery(&id).unwrap());
        drop(db);
        let db = reopen(&path);
        db.recover_message_deliveries("boot-2").unwrap();
        assert_eq!(db.pending_message_deliveries().unwrap()[0].id, id);
        publish(&db, "task-a", "DONE");
        assert_eq!(db.pending_message_deliveries().unwrap().len(), 1);
    }
    #[test]
    fn delegation_delivery_blocked_sibling_never_holds_per_task_publication() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.sqlite");
        let db = reopen(&path);
        for (id, child) in [("a", "child-a"), ("b", "child-b"), ("c", "child-c")] {
            db.admit_delegation_task(&task(id, child), None, &json!({"id":id}))
                .unwrap();
        }
        db.update_delegation_task("a", |t| {
            t.status = TaskStatus::Waiting;
            t.pending_request = Some(PendingRequest {
                id: "question".into(),
                kind: "user_input".into(),
                created_at: 1,
            });
        })
        .unwrap();
        publish(&db, "b", "Success");
        db.publish_delegation_result(
            "c",
            &PublishedResult {
                version: 1,
                run_id: None,
                text: "Failed".into(),
                status: TaskStatus::Failed,
                published_at: 2,
            },
            &payload("c", "Failed"),
            "boot-1",
            None,
        )
        .unwrap();
        drop(db);
        let db = reopen(&path);
        assert_eq!(db.pending_message_deliveries().unwrap().len(), 2);
        assert!(db.delegation_task("a").unwrap().unwrap().result.is_none());
        assert_eq!(
            db.delegation_task("a")
                .unwrap()
                .unwrap()
                .pending_request
                .unwrap()
                .id,
            "question"
        );
    }
    #[test]
    fn delegation_delivery_same_key_concurrent_retry_changed_payload_and_crash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.sqlite");
        let db = Arc::new(reopen(&path));
        let barrier = Arc::new(Barrier::new(3));
        let jobs: Vec<_> = (0..2)
            .map(|i| {
                let db = db.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    db.admit_delegation_task(
                        &task(&format!("task-{i}"), &format!("child-{i}")),
                        Some("same-key"),
                        &json!({"task":"review"}),
                    )
                    .unwrap()
                })
            })
            .collect();
        barrier.wait();
        let a = jobs
            .into_iter()
            .map(|j| j.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(a[0].0.task_id, a[1].0.task_id);
        assert_eq!(a[0].0.child_thread_id, a[1].0.child_thread_id);
        assert_ne!(a[0].1, a[1].1);
        drop(db);
        let db = reopen(&path);
        let retry = db
            .admit_delegation_task(
                &task("another", "other"),
                Some("same-key"),
                &json!({"task":"review"}),
            )
            .unwrap();
        assert!(!retry.1);
        assert_eq!(retry.0.task_id, a[0].0.task_id);
        assert_eq!(db.delegation_tasks(None).unwrap().len(), 1);
        assert!(db
            .admit_delegation_task(
                &task("changed", "changed"),
                Some("same-key"),
                &json!({"task":"implement"})
            )
            .unwrap_err()
            .contains("conflict"));
    }
    #[test]
    fn delegation_delivery_stop_durable_includes_worktree_later_runs_and_retains_results() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.sqlite");
        let db = reopen(&path);
        let mut worktree = task("worktree", "worktree-child");
        worktree.isolation = "worktree".into();
        db.admit_delegation_task(&worktree, None, &json!({}))
            .unwrap();
        let mut nested = task("nested", "nested-child");
        nested.parent_thread_id = "worktree-child".into();
        db.admit_delegation_task(&nested, None, &json!({})).unwrap();
        let mut independent = task("top-level", "other-child");
        independent.parent_thread_id = "other-parent".into();
        db.admit_delegation_task(&independent, None, &json!({}))
            .unwrap();
        db.arm_pr_watch("worktree-child", "workspace", 7, None, 3, 1)
            .unwrap();
        publish(&db, "worktree", "Original final result");
        db.update_delegation_task("worktree", |t| {
            t.child_run_ids.push("later-run".into());
            t.has_pending_child_runs = true;
        })
        .unwrap();
        let mut late = self::task("late", "late-child");
        late.admission_generation = Some(db.delivery_generation("parent").unwrap().0);
        db.stop_message_deliveries("parent").unwrap();
        assert_eq!(db.stop_delegation_tree("parent").unwrap().len(), 2);
        drop(db);
        let db = reopen(&path);
        let task = db.delegation_task("worktree").unwrap().unwrap();
        assert_eq!(task.result.unwrap().text, "Original final result");
        assert_eq!(task.completion_delivery, "disposed_by_stop");
        assert_eq!(
            db.get_pr_watch("worktree-child").unwrap().unwrap().status,
            "ended"
        );
        assert_eq!(
            db.delegation_task("nested")
                .unwrap()
                .unwrap()
                .progress
                .as_deref(),
            Some("Stopping; provider confirmation pending")
        );
        assert_eq!(
            db.delegation_task("top-level")
                .unwrap()
                .unwrap()
                .cancellation_generation,
            0
        );
        assert!(db.pending_message_deliveries().unwrap().is_empty());
        assert!(db
            .admit_delegation_task(&late, None, &json!({}))
            .is_err());
    }
    #[test]
    fn delegation_delivery_complete_status_ack_is_owned_and_fences_claimed_queue() {
        let db = DatabaseStore::new_in_memory();
        db.admit_delegation_task(&task("a", "child"), None, &json!({}))
            .unwrap();
        publish(&db, "a", "Final");
        let delivery = db.pending_message_deliveries().unwrap()[0].id.clone();
        assert!(db.claim_message_delivery(&delivery).unwrap());
        assert!(db.acknowledge_delegation_result("other", "a").is_err());
        assert_eq!(
            db.acknowledge_delegation_result("parent", "a")
                .unwrap()
                .result
                .unwrap()
                .text,
            "Final"
        );
        assert!(db.message_delivery_submitting(&delivery).is_err());
        assert_eq!(
            db.delegation_task("a")
                .unwrap()
                .unwrap()
                .completion_delivery,
            "acknowledged"
        );
    }
    #[test]
    fn delegation_delivery_immutable_result_survives_later_runs_and_import_replay() {
        let db = DatabaseStore::new_in_memory();
        db.admit_delegation_task(&task("a", "child"), None, &json!({}))
            .unwrap();
        publish(&db, "a", "Original");
        db.update_delegation_task("a", |t| {
            t.result = None;
            t.child_run_ids.push("later".into());
            t.has_pending_child_runs = true;
        })
        .unwrap();
        assert_eq!(
            db.delegation_task("a")
                .unwrap()
                .unwrap()
                .result
                .unwrap()
                .text,
            "Original"
        );
        db.publish_delegation_result(
            "a",
            &PublishedResult {
                version: 1,
                run_id: None,
                text: "Changed".into(),
                status: TaskStatus::Failed,
                published_at: 3,
            },
            &payload("a", "Changed"),
            "boot-2",
            Some(("remote-a", 10)),
        )
        .unwrap();
        assert_eq!(db.pending_message_deliveries().unwrap().len(), 1);
        assert_eq!(
            db.delegation_task("a")
                .unwrap()
                .unwrap()
                .result
                .unwrap()
                .text,
            "Original"
        );
    }
    #[test]
    fn delegation_delivery_wait_release_and_dispose_after_claim_never_reopen_delivery() {
        let db = DatabaseStore::new_in_memory();
        let mut t = task("a", "child");
        t.wait_until = Some(4_000_000_000_000);
        db.admit_delegation_task(&t, None, &json!({})).unwrap();
        publish(&db, "a", "Final");
        assert!(db.pending_message_deliveries().unwrap().is_empty());
        db.release_delegation_wait("a").unwrap();
        let id = db.pending_message_deliveries().unwrap()[0].id.clone();
        assert!(db.claim_message_delivery(&id).unwrap());
        db.dispose_delegation_task("parent", "a").unwrap();
        assert!(db.message_delivery_submitting(&id).is_err());
        publish(&db, "a", "Final");
        assert!(db.pending_message_deliveries().unwrap().is_empty());
    }
}

#[cfg(test)]
mod native_budget_tests {
    use super::*;
    fn record(
        db: &DatabaseStore,
        parent: &str,
        child: &str,
        id: &str,
        lineage: Vec<String>,
    ) -> TaskRecord {
        let root = db.orchestration_context(parent).unwrap();
        let mut task = TaskRecord::new(
            id.into(),
            parent.into(),
            Some(child.into()),
            "claude".into(),
            id.into(),
            "A self-contained complete brief".into(),
            1,
        );
        task.budget_root_id = Some(root.root_id);
        task.lineage = lineage;
        task
    }
    #[test]
    fn native_budgets_survive_agent_sends_and_database_reopen() {
        let db = DatabaseStore::new_in_memory();
        db.orchestration_human_root("parent").unwrap();
        let root = db.orchestration_context("parent").unwrap().root_id;
        for i in 0..6 {
            let id = format!("task-{i}");
            let child = format!("child-{i}");
            let task = record(&db, "parent", &child, &id, vec![]);
            db.admit_delegation_task(&task, Some(&id), &serde_json::json!({"id":id}))
                .unwrap();
            db.update_delegation_task(&id, |t| t.status = TaskStatus::Completed)
                .unwrap();
        }
        let next = record(&db, "parent", "extra-child", "extra", vec![]);
        assert!(db
            .admit_delegation_task(&next, None, &Value::Null)
            .unwrap_err()
            .contains("resource_limit"));
        let dir = tempfile::tempdir().unwrap();
        let db = db.control_fixture_snapshot(&dir.path().join("root.sqlite"));
        assert_eq!(db.orchestration_context("parent").unwrap().root_id, root);
        assert!(db
            .admit_delegation_task(&next, None, &Value::Null)
            .unwrap_err()
            .contains("resource_limit"));
        db.orchestration_human_root("parent").unwrap();
        assert_ne!(db.orchestration_context("parent").unwrap().root_id, root);
    }
    #[test]
    fn native_nested_depth_is_acyclic_and_execution_capacity_is_shared() {
        let db = DatabaseStore::new_in_memory();
        db.orchestration_human_root("root").unwrap();
        let mut parent = "root".to_string();
        let mut lineage = vec![];
        for depth in 0..4 {
            let child = format!("nested-{depth}");
            let id = format!("task-{depth}");
            let mut task = record(&db, &parent, &child, &id, lineage.clone());
            task.wait_until=Some(chrono::Utc::now().timestamp_millis()+20_000);
            db.admit_delegation_task(&task, None, &serde_json::json!({"mode":"wait"}))
                .unwrap();
            lineage.push(id);
            parent = child;
        }
        let next = record(&db, &parent, "too-deep", "too-deep-task", lineage);
        assert!(db
            .admit_delegation_task(&next, None, &Value::Null)
            .unwrap_err()
            .contains("resource_limit"));
        let stopped = db.stop_delegation_tree("root").unwrap();
        assert_eq!(stopped.len(), 4);
    }
    #[test]
    fn native_followup_admission_keeps_original_result_and_deduplicates() {
        let db = DatabaseStore::new_in_memory();
        db.orchestration_human_root("parent").unwrap();
        let task = record(&db, "parent", "child", "original", vec![]);
        db.admit_delegation_task(&task, None, &Value::Null).unwrap();
        db.update_delegation_task("original", |task| {
            task.status = TaskStatus::Completed;
            task.result = Some(PublishedResult {
                version: 1,
                run_id: Some("original-run".into()),
                text: "Original final result".into(),
                status: TaskStatus::Completed,
                published_at: 2,
            });
        })
        .unwrap();
        let payload = serde_json::json!({"origin":"controller","caller":{"identity":{"kind":"thread","thread_id":"parent"}},"provenance":{"created_by":"agent"},"input":{"text":"Follow up with another complete task"}});
        assert!(
            db.admit_message_delivery(
                "followup",
                "child",
                "followup-message",
                &payload,
                "epoch",
                0,
                None,
                false
            )
            .unwrap()
            .1
        );
        assert!(
            !db.admit_message_delivery(
                "followup",
                "child",
                "followup-message",
                &payload,
                "epoch",
                0,
                None,
                false
            )
            .unwrap()
            .1
        );
        let original = db.delegation_task("original").unwrap().unwrap();
        assert_eq!(original.result.unwrap().text, "Original final result");
        let followup = db
            .delegation_task("local:followup:followup")
            .unwrap()
            .unwrap();
        assert!(followup.result.is_none());
        assert_eq!(followup.child_thread_id.as_deref(), Some("child"));
        assert_eq!(db.delegation_tasks(Some("parent")).unwrap().len(), 2);
    }
    #[test]
    fn native_read_ack_requires_contiguous_complete_owned_coverage() {
        let db = DatabaseStore::new_in_memory();
        let task = record(&db, "parent", "child", "read-task", vec![]);
        db.admit_delegation_task(&task, None, &Value::Null).unwrap();
        assert!(!db
            .delegation_read_coverage("parent", "read-task", 0, 4, 10)
            .unwrap());
        assert!(!db
            .delegation_read_coverage("parent", "read-task", 7, 10, 10)
            .unwrap());
        assert!(db
            .delegation_read_coverage("parent", "read-task", 4, 7, 10)
            .unwrap());
        assert!(db
            .delegation_read_coverage("unrelated", "read-task", 0, 10, 10)
            .is_err());
    }
    #[test]
    fn native_agent_message_retries_do_not_spend_work_or_replace_recipient_root() {
        let db = DatabaseStore::new_in_memory();
        db.orchestration_human_root("a").unwrap();
        db.orchestration_human_root("b").unwrap();
        let root = db.orchestration_context("b").unwrap().root_id;
        for i in 0..30 {
            let id = format!("send-{i}");
            let payload = serde_json::json!({"origin":"controller","input":{"text":id},"caller":{"identity":{"kind":"thread","thread_id":"a"}},"provenance":{"created_by":"agent"}});
            assert!(db.admit_message_delivery(&id,"b",&id,&payload,"epoch",0,None,false).unwrap().1);
            assert!(!db.admit_message_delivery(&id,"b",&id,&payload,"epoch",0,None,false).unwrap().1);
            assert!(db.admit_message_delivery(&format!("duplicate-{i}"),"b",&format!("duplicate-{i}"),&payload,"epoch",0,None,false).unwrap_err().contains("duplicate_causation"));
        }
        assert_eq!(db.orchestration_context("b").unwrap().root_id,root);
        let conn=db.conn.lock().unwrap();
        let count:u32=conn.query_row("SELECT COUNT(*) FROM agent_orchestration_admissions",[],|r|r.get(0)).unwrap();
        assert_eq!(count,0);
    }
    #[test]
    fn native_waiting_ancestors_yield_and_release_their_execution_slots() {
        let db=DatabaseStore::new_in_memory();
        let mut parent="root".to_string();
        for i in 0..4 {
            let id=format!("wait-{i}");let child=format!("child-{i}");
            let mut task=record(&db,&parent,&child,&id,vec![]);
            task.wait_until=Some(chrono::Utc::now().timestamp_millis()+20_000);
            db.admit_delegation_task(&task,None,&serde_json::json!({"mode":"wait"})).unwrap();
            parent=child;
        }
        assert_eq!(db.delegation_task("wait-0").unwrap().unwrap().work_state,WorkState::WaitingForChildren);
        db.release_delegation_wait("wait-1").unwrap();
        assert_eq!(db.delegation_task("wait-0").unwrap().unwrap().work_state,WorkState::Working);
        let conn=db.conn.lock().unwrap();
        let plan:String=conn.query_row("EXPLAIN QUERY PLAN SELECT task_id FROM agent_delegation_tasks WHERE json_extract(record,'$.budget_root_id')='root' AND json_extract(record,'$.status')='running'",[],|r|r.get(3)).unwrap();
        assert!(plan.contains("delegation_task_budget"),"{plan}");
    }
    #[test]
    fn native_batch_launches_reports_and_later_human_rounds_keep_separate_budgets() {
        let db=DatabaseStore::new_in_memory();
        db.orchestration_human_root("coordinator").unwrap();
        let original=db.orchestration_context("coordinator").unwrap().root_id;
        for i in 0..3 {
            let child=format!("launch-{i}");
            db.admit_control_operation_for_thread("thread:coordinator",&child,"thread_launch",&child,"epoch",Some("workspace"),Some(&child),Some("coordinator")).unwrap();
            db.orchestration_launch_finished(&child).unwrap();
        }
        for i in 0..3 {
            let id=format!("report-{i}");let child=format!("launch-{i}");
            let payload=serde_json::json!({"origin":"controller","input":{"text":id},"caller":{"identity":{"kind":"thread","thread_id":child}},"provenance":{"created_by":"agent"}});
            db.admit_message_delivery(&id,"coordinator",&id,&payload,"epoch",0,None,false).unwrap();
        }
        for i in 3..6 {
            let child=format!("launch-{i}");
            db.admit_control_operation_for_thread("thread:coordinator",&child,"thread_launch",&child,"epoch",Some("workspace"),Some(&child),Some("coordinator")).unwrap();
            db.orchestration_launch_finished(&child).unwrap();
        }
        assert!(db.admit_control_operation_for_thread("thread:coordinator","seventh","thread_launch","seventh","epoch",Some("workspace"),Some("seventh"),Some("coordinator")).err().unwrap().contains("resource_limit"));
        db.orchestration_human_root("coordinator").unwrap();
        let new_root=db.orchestration_context("coordinator").unwrap().root_id;
        let payload=serde_json::json!({"origin":"controller","input":{"text":"Late result from earlier round"},"caller":{"identity":{"kind":"thread","thread_id":"launch-0"}},"provenance":{"created_by":"agent"}});
        db.admit_message_delivery("late","coordinator","late",&payload,"epoch",0,None,false).unwrap();
        assert_eq!(db.orchestration_context("coordinator").unwrap().root_id,new_root);
        assert_ne!(new_root,original);
    }
    #[test]
    fn native_task_start_uses_reserved_budget_and_preserves_child_context() {
        let db = DatabaseStore::new_in_memory();
        db.orchestration_human_root("parent").unwrap();
        let root = db.orchestration_context("parent").unwrap().root_id;
        for i in 0..6 {
            let child = format!("child-{i}");
            let id = format!("task-{i}");
            let task = record(&db, "parent", &child, &id, vec![]);
            db.admit_delegation_task(&task, None, &Value::Null).unwrap();
            let payload = serde_json::json!({"origin":"delegation","caller":{"identity":{"kind":"thread","thread_id":"parent"},"admitted":{"kind":"task","task_id":id}},"provenance":{"created_by":"agent"}});
            db.admit_message_delivery(&format!("delivery-{i}"), &child, &format!("message-{i}"), &payload, "epoch", 0, None, false).unwrap();
            let context = db.orchestration_context(&child).unwrap();
            assert_eq!(context.root_id, root);
            assert_eq!(context.depth, 1);
            assert_eq!(context.causation_id, id);
            db.update_delegation_task(&id, |task| task.status = TaskStatus::Completed).unwrap();
        }
        let extra = record(&db, "parent", "extra", "extra", vec![]);
        assert!(db.admit_delegation_task(&extra, None, &Value::Null).unwrap_err().contains("resource_limit"));
        let wrong_target = serde_json::json!({"origin":"delegation","caller":{"identity":{"kind":"thread","thread_id":"parent"},"admitted":{"kind":"task","task_id":"task-0"}},"provenance":{"created_by":"agent"}});
        assert!(db.admit_message_delivery("spoof", "other", "spoof", &wrong_target, "epoch", 0, None, false).unwrap().1);
        assert!(db.orchestration_context("other").unwrap().root_id != root);
    }
}
