//! Journal only committed core history, with independent, durable plugin delivery cursors.
use super::{
    platform::PlatformService,
    platform_authority::{self, PlatformCaller},
    platform_store::InstallationRecord,
};
use crate::database::DatabaseStore;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Mutex};
use tauri::{AppHandle, Manager, Runtime};

pub struct PlatformEvents {
    path: PathBuf,
    gate: Mutex<()>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunFinalizedEvent {
    pub sequence: i64,
    pub delivery_id: String,
    pub thread_id: String,
    pub project_id: String,
    pub run_id: String,
    pub outcome: String,
    pub summary: String,
    /// Automatic effects require committed input-to-run provenance; uncorrelated legacy runs remain observable.
    pub effects_allowed: bool,
}
impl PlatformEvents {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            gate: Mutex::new(()),
        }
    }
    fn connection(&self) -> Result<Connection, String> {
        super::platform_store::private_directory(
            self.path.parent().ok_or("Invalid event store path")?,
        )?;
        if let Ok(meta) = std::fs::symlink_metadata(&self.path) {
            if meta.file_type().is_symlink() {
                return Err("storage_failed: Event journal must not be a symlink".into());
            }
        }
        let connection = Connection::open(&self.path).map_err(|_| {
            "storage_failed: Plugin event journal is unavailable; core history is unaffected"
        })?;
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        if version > 1 {
            return Err("incompatible: Plugin event journal belongs to a newer host".into());
        }
        connection
            .execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
            .map_err(|e| e.to_string())?;
        connection.execute_batch("CREATE TABLE IF NOT EXISTS finalized(sequence INTEGER PRIMARY KEY AUTOINCREMENT, delivery_id TEXT UNIQUE NOT NULL,thread_id TEXT NOT NULL,project_id TEXT NOT NULL,run_id TEXT NOT NULL,outcome TEXT NOT NULL,summary TEXT NOT NULL); CREATE TABLE IF NOT EXISTS captured(thread_id TEXT PRIMARY KEY,row_id INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS run_sources(thread_id TEXT NOT NULL,run_id TEXT NOT NULL,plugin_caused INTEGER NOT NULL,PRIMARY KEY(thread_id,run_id)); PRAGMA user_version=1;").map_err(|e|e.to_string())?;
        Ok(connection)
    }
    pub fn latest(&self) -> Result<i64, String> {
        let _gate = self.gate.lock().map_err(|_| "Plugin event lock failed")?;
        self.connection()?
            .query_row("SELECT COALESCE(MAX(sequence),0) FROM finalized", [], |r| {
                r.get(0)
            })
            .map_err(|e| e.to_string())
    }
    /// Call after core persistence commits. Rereading history prevents an uncommitted provider tap from becoming an event.
    pub fn recover_thread(
        &self,
        db: &DatabaseStore,
        project: &str,
        thread: &str,
    ) -> Result<usize, String> {
        let commands = self.command_keys();
        self.recover_thread_with_commands(db, project, thread, &commands)
    }
    fn command_keys(&self) -> std::collections::BTreeSet<String> {
        super::platform_store::PlatformStore::new(
            self.path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .to_path_buf(),
        )
        .list()
        .into_iter()
        .filter_map(|(_, record)| record.ok())
        .flat_map(|record| {
            record
                .receipts
                .into_values()
                .map(|receipt| receipt.command_key)
        })
        .collect()
    }
    fn recover_thread_with_commands(
        &self,
        db: &DatabaseStore,
        project: &str,
        thread: &str,
        commands: &std::collections::BTreeSet<String>,
    ) -> Result<usize, String> {
        let _gate = self.gate.lock().map_err(|_| "Plugin event lock failed")?;
        let mut connection = self.connection()?;
        let captured_cursor: i64 = connection
            .query_row(
                "SELECT row_id FROM captured WHERE thread_id=?1",
                params![thread],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
            .unwrap_or(0);
        // A fork's copied transcript is historical even when this journal has never
        // seen the thread. Recheck its boundary on every sweep, including stale cursors.
        let cursor = captured_cursor.max(
            db.agent_chat_fork_origin(thread)
                .and_then(|origin| origin.boundary_message_id)
                .unwrap_or(0),
        );
        let mut before = i64::MAX;
        let mut through = cursor;
        let mut finalized = Vec::new();
        let mut sources = Vec::new();
        loop {
            // The existing reverse cursor query bounds transcript payload memory.
            // Only compact completion/provenance metadata survives each page.
            let rows = db
                .list_agent_chat_messages_before(thread, before, 64)
                .map_err(|e| format!("storage_failed: Core event recovery read failed: {e}"))?;
            let Some((oldest, _, _)) = rows.first() else {
                break;
            };
            if let Some((newest, _, _)) = rows.last() {
                through = through.max(*newest);
            }
            for (row, payload, _) in &rows {
                if *row <= cursor {
                    continue;
                }
                let Ok(value) = serde_json::from_str::<Value>(payload) else {
                    continue;
                };
                if value["type"] == "native_user_control" && value["steered"] != true {
                    if let (Some(user_row), Some(run)) =
                        (value["user_message_id"].as_i64(), value["turn_id"].as_str())
                    {
                        if let Some(user) = db
                            .get_agent_chat_message(user_row)
                            .and_then(|payload| serde_json::from_str::<Value>(&payload).ok())
                        {
                            let nonce = user["client_nonce"]
                                .as_str()
                                .or(user["message_id"].as_str());
                            sources.push((
                                run.to_string(),
                                nonce.is_some_and(|nonce| commands.contains(nonce)
                                    // This host-reserved namespace is also conservatively
                                    // suppressed if an effect commits during the sweep.
                                    || nonce.strip_prefix("plugin-").is_some_and(|key| key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit()))),
                            ));
                        }
                    }
                }
                if value["type"] != "turn_completed" {
                    continue;
                }
                let Some(run) = value["turn_id"].as_str().filter(|v| !v.is_empty()) else {
                    continue;
                };
                let outcome = match (
                    value["status"]["kind"].as_str(),
                    value["status"]["subtype"].as_str(),
                ) {
                    (Some("completed" | "success"), _) => "completed",
                    (Some("error"), Some("interrupted"))
                    | (Some("interrupted" | "cancelled" | "canceled"), _) => "interrupted",
                    (Some("failed" | "error" | "max_turns" | "max_budget"), _) => "failed",
                    _ => continue,
                };
                finalized.push((*row, run.to_string(), outcome));
            }
            if *oldest <= cursor || rows.len() < 64 {
                break;
            }
            before = *oldest;
        }
        finalized.sort_by_key(|(row, _, _)| *row);
        let tx = connection.transaction().map_err(|e| e.to_string())?;
        for (run, plugin_caused) in sources {
            tx.execute("INSERT INTO run_sources(thread_id,run_id,plugin_caused) VALUES(?1,?2,?3) ON CONFLICT(thread_id,run_id) DO UPDATE SET plugin_caused=MAX(plugin_caused,excluded.plugin_caused)",params![thread,run,plugin_caused]).map_err(|e|e.to_string())?;
        }
        let mut count = 0;
        for (row, run, outcome) in finalized {
            let delivery = super::platform_store::digest(&json!([thread, run]));
            count+=tx.execute("INSERT OR IGNORE INTO finalized(delivery_id,thread_id,project_id,run_id,outcome,summary) VALUES(?1,?2,?3,?4,?5,?6)",params![format!("run-{delivery}"),thread,project,run,outcome,format!("Run {outcome} (committed history row {row}).")]).map_err(|e|e.to_string())?;
        }
        if through > captured_cursor {
            tx.execute("INSERT INTO captured(thread_id,row_id) VALUES(?1,?2) ON CONFLICT(thread_id) DO UPDATE SET row_id=MAX(row_id,excluded.row_id)",params![thread,through]).map_err(|e|e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(count)
    }
    /// Startup reconciliation is read-only against core state and can be retried after journal failures.
    pub fn recover_committed<R: Runtime>(&self, app: &AppHandle<R>) -> Result<usize, String> {
        let db = app.state::<DatabaseStore>();
        let mut count = 0;
        // Decode retained plugin ledgers once per sweep, rather than once per thread.
        let commands = self.command_keys();
        for (workspace, project) in super::platform_workflow::recovery_workspaces(app) {
            let mut cursor = None;
            loop {
                let ids = db.control_thread_ids(&workspace, cursor.as_deref(), 100)?;
                for id in &ids {
                    count += self.recover_thread_with_commands(&db, &project, id, &commands)?;
                }
                if ids.len() < 100 {
                    break;
                }
                cursor = ids.last().cloned();
            }
        }
        Ok(count)
    }
    fn page(
        &self,
        after: i64,
        through: i64,
        limit: usize,
    ) -> Result<Vec<RunFinalizedEvent>, String> {
        let _gate = self.gate.lock().map_err(|_| "Plugin event lock failed")?;
        let connection = self.connection()?;
        let mut stmt=connection.prepare("SELECT f.sequence,f.delivery_id,f.thread_id,f.project_id,f.run_id,f.outcome,f.summary,s.plugin_caused IS NOT NULL FROM finalized f LEFT JOIN run_sources s ON s.thread_id=f.thread_id AND s.run_id=f.run_id WHERE f.sequence>?1 AND f.sequence<=?2 AND COALESCE(s.plugin_caused,0)=0 ORDER BY f.sequence LIMIT ?3").map_err(|e|e.to_string())?;
        let rows = stmt
            .query_map(params![after, through, limit as i64], |r| {
                Ok(RunFinalizedEvent {
                    sequence: r.get(0)?,
                    delivery_id: r.get(1)?,
                    thread_id: r.get(2)?,
                    project_id: r.get(3)?,
                    run_id: r.get(4)?,
                    outcome: r.get(5)?,
                    summary: r.get(6)?,
                    effects_allowed: r.get(7)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }
    fn eligible(record: &InstallationRecord, event: &RunFinalizedEvent) -> bool {
        record.grants.project_ids.contains(&event.project_id)
            && event.sequence
                > *record
                    .project_grant_starts
                    .get(&event.project_id)
                    .unwrap_or(&record.event_cursor)
            && !record
                .acknowledged_deliveries
                .contains_key(&event.delivery_id)
    }
    pub fn backlog(&self, record: &InstallationRecord) -> Result<Value, String> {
        let through = self.latest()?;
        let mut cursor = record.event_cursor;
        let mut count = 0;
        loop {
            let page = self.page(cursor, through, 200)?;
            if page.is_empty() {
                break;
            }
            for event in &page {
                if Self::eligible(record, event) {
                    count += 1;
                }
            }
            cursor = page.last().map(|e| e.sequence).unwrap_or(cursor);
        }
        Ok(
            json!({"count":count,"afterSequence":record.event_cursor,"throughSequence":through,"requiresChoice":record.event_backlog_choice_required,"error":record.delivery_error}),
        )
    }
    pub fn recover(
        &self,
        store: &super::platform_store::PlatformStore,
        id: &str,
        choice: &str,
    ) -> Result<InstallationRecord, String> {
        if !matches!(choice, "replay" | "skip") {
            return Err("invalid_input: Event recovery requires replay or skip".into());
        }
        let latest = self.latest()?;
        store.update(id, |record| {
            if choice == "skip" {
                record.event_cursor = latest;
            }
            record.event_backlog_choice_required = false;
            record.delivery_error = None;
            Ok(record.clone())
        })
    }
    /// Empty gaps need no replay decision; future live events must not stay paused forever.
    pub fn clear_empty_gap(
        &self,
        store: &super::platform_store::PlatformStore,
        id: &str,
    ) -> Result<InstallationRecord, String> {
        let record = store.read(id)?;
        if !record.event_backlog_choice_required {
            return Ok(record);
        }
        let backlog = self.backlog(&record)?;
        if backlog["count"].as_u64() != Some(0) {
            return Ok(record);
        }
        let through = backlog["throughSequence"]
            .as_i64()
            .ok_or("storage_failed: Event position is unavailable")?;
        store.update(id, |current| {
            if current.generation != record.generation {
                return Err("stale_generation: Plugin changed during event recovery".into());
            }
            current.event_cursor = current.event_cursor.max(through);
            current.event_backlog_choice_required = false;
            current.delivery_error = None;
            Ok(current.clone())
        })
    }
    pub fn pause(
        &self,
        store: &super::platform_store::PlatformStore,
        id: &str,
        error: String,
    ) -> Result<(), String> {
        store.update(id, |record| {
            record.delivery_error = Some(error);
            record.event_backlog_choice_required = true;
            Ok(())
        })
    }
    pub fn dispatch<R: Runtime>(
        &self,
        service: &PlatformService,
        app: &AppHandle<R>,
        caller: &PlatformCaller,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        let record = service.authorize(app, caller, "events.run-finalized", None)?;
        if method == "events.backlog" {
            return self.backlog(&record);
        }
        if record.event_backlog_choice_required || record.delivery_error.is_some() {
            return Err(
                "backlog_choice_required: Review replay or skip before delivering events".into(),
            );
        }
        if method == "events.ack" {
            let delivery = super::platform::string_field(&params, "deliveryId", 128)?;
            if record.acknowledged_deliveries.contains_key(delivery) {
                return Ok(Value::Null);
            }
            let next = self.next(&record, caller)?;
            let Some(event) = next else {
                return Err("invalid_input: No delivery is awaiting acknowledgement".into());
            };
            if event.delivery_id != delivery {
                return Err("invalid_input: Acknowledge the next pending delivery only".into());
            }
            service.store.update(&caller.installation_id, |record| {
                platform_authority::capability(
                    record,
                    caller,
                    "events.run-finalized",
                    Some(&event.project_id),
                )?;
                record
                    .acknowledged_deliveries
                    .insert(delivery.into(), event.sequence);
                record.event_cursor = record.event_cursor.max(event.sequence);
                Ok(Value::Null)
            })
        } else {
            serde_json::to_value(self.next(&record, caller)?).map_err(|e| e.to_string())
        }
    }
    pub fn next(
        &self,
        record: &InstallationRecord,
        caller: &PlatformCaller,
    ) -> Result<Option<RunFinalizedEvent>, String> {
        let through = self.latest()?;
        let mut cursor = record.event_cursor;
        loop {
            let page = self.page(cursor, through, 200)?;
            if page.is_empty() {
                return Ok(None);
            }
            for event in &page {
                if Self::eligible(record, event) {
                    if caller
                        .project_ids
                        .as_ref()
                        .is_some_and(|ids| !ids.contains(&event.project_id))
                    {
                        return Err("permission_denied: The next event is outside this actor's project scope".into());
                    }
                    return Ok(Some(event.clone()));
                }
            }
            cursor = page.last().map(|e| e.sequence).unwrap_or(cursor);
        }
    }
}
