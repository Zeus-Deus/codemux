//! Canonical task identity, immutable results, and completion ownership.
//! No session FK: task lineage/results must survive closing or deleting a pane.
use super::{message_delivery, DatabaseStore};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(super) fn create_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS agent_delegation_tasks (
        task_id TEXT PRIMARY KEY, parent_thread_id TEXT NOT NULL,
        child_thread_id TEXT, backend TEXT NOT NULL, client_request_id TEXT,
        admission_payload TEXT NOT NULL, record TEXT NOT NULL,
        UNIQUE(parent_thread_id, client_request_id));
        CREATE INDEX IF NOT EXISTS delegation_task_child ON agent_delegation_tasks(child_thread_id);
        CREATE INDEX IF NOT EXISTS delegation_task_parent ON agent_delegation_tasks(parent_thread_id);
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
impl DatabaseStore {
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
        }
        Ok(())
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
                if serde_json::from_str::<Value>(&old).ok().as_ref() != Some(payload) {
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
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        if let Some(prior) = read(&tx, &task.task_id)? {
            return Ok((prior, false));
        }
        if let Some(key) = key {
            let prior:Option<(String,String)>=tx.query_row("SELECT record,admission_payload FROM agent_delegation_tasks WHERE parent_thread_id=?1 AND client_request_id=?2",params![task.parent_thread_id,key],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|e.to_string())?;
            if let Some((data, old)) = prior {
                if serde_json::from_str::<Value>(&old).ok().as_ref() != Some(payload) {
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
        if task.backend == TaskBackend::Local {
            if let Some(child) = &task.child_thread_id {
                tx.execute("INSERT INTO agent_chat_sessions (thread_id,workspace_id,cwd,provider,title,model,effort,permission_mode) VALUES (?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(thread_id) DO NOTHING",params![child,task.workspace_id,task.cwd,task.provider,task.title,task.model,task.effort,task.permission_mode]).map_err(|e|e.to_string())?;
            }
        }
        tx.execute("INSERT INTO agent_delegation_tasks (task_id,parent_thread_id,child_thread_id,backend,client_request_id,admission_payload,record) VALUES (?1,?2,?3,?4,?5,?6,?7)",params![task.task_id,task.parent_thread_id,task.child_thread_id,if task.backend==TaskBackend::Local {"local"}else{"ssh"},key,payload.to_string(),serde_json::to_string(&task).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
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
