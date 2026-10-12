use std::{path::Path, sync::Mutex, time::Duration};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use super::{RunSnapshot, RunSummary, RunSummarySpec, WorkflowEvent};

pub(super) struct Store {
    pub connection: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let connection = Connection::open(path).map_err(|e| e.to_string())?;
        connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
             CREATE TABLE IF NOT EXISTS workflow_runs(id TEXT PRIMARY KEY, snapshot TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'running',workspace_id TEXT NOT NULL DEFAULT '',updated_at_ms INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE IF NOT EXISTS workflow_run_summaries(id TEXT PRIMARY KEY,status TEXT NOT NULL,revision INTEGER NOT NULL,
                created_at_ms INTEGER NOT NULL,updated_at_ms INTEGER NOT NULL,workspace_id TEXT NOT NULL,title TEXT NOT NULL,mode TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS workflow_operations(
                scope TEXT NOT NULL, key TEXT NOT NULL, payload TEXT NOT NULL, run_id TEXT NOT NULL,
                PRIMARY KEY(scope,key));
             CREATE INDEX IF NOT EXISTS workflow_operations_run ON workflow_operations(run_id);
             CREATE TABLE IF NOT EXISTS workflow_journal(
                run_id TEXT NOT NULL, command_id TEXT NOT NULL, payload TEXT NOT NULL,
                result TEXT, error TEXT, PRIMARY KEY(run_id,command_id));
             CREATE TABLE IF NOT EXISTS workflow_events(
                sequence INTEGER PRIMARY KEY AUTOINCREMENT, run_id TEXT NOT NULL,
                revision INTEGER NOT NULL, kind TEXT NOT NULL, timestamp_ms INTEGER NOT NULL);",
            )
            .map_err(|e| e.to_string())?;
        let has_counters: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='workflow_operation_counters')",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !has_counters {
            // Schema and backfill commit together: reopening after a crash
            // cannot mistake an empty counter table for a completed migration.
            connection.execute_batch("BEGIN IMMEDIATE;
                CREATE TABLE workflow_operation_counters(run_id TEXT PRIMARY KEY,retained_bytes INTEGER NOT NULL,operation_count INTEGER NOT NULL);
                INSERT INTO workflow_operation_counters(run_id,retained_bytes,operation_count)
                    SELECT run_id,COALESCE(SUM(length(CAST(payload AS BLOB))+length(CAST(key AS BLOB))),0),COUNT(*) FROM workflow_operations GROUP BY run_id;
                COMMIT;").map_err(|e|e.to_string())?;
        }
        let has_journal_counters: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='workflow_journal_counters')",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if !has_journal_counters {
            connection.execute_batch("BEGIN IMMEDIATE;
                CREATE TABLE workflow_journal_counters(run_id TEXT PRIMARY KEY,retained_bytes INTEGER NOT NULL,entry_count INTEGER NOT NULL);
                INSERT INTO workflow_journal_counters(run_id,retained_bytes,entry_count)
                    SELECT run_id,COALESCE(SUM(length(CAST(command_id AS BLOB))+length(CAST(payload AS BLOB))+COALESCE(length(CAST(result AS BLOB)),0)+COALESCE(length(CAST(error AS BLOB)),0)),0),COUNT(*) FROM workflow_journal GROUP BY run_id;
                COMMIT;").map_err(|e|e.to_string())?;
        }
        let columns = {
            let mut statement = connection
                .prepare("PRAGMA table_info(workflow_runs)")
                .map_err(|e| e.to_string())?;
            let rows = statement
                .query_map([], |r| r.get::<_, String>(1))
                .map_err(|e| e.to_string())?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?
        };
        for (name, sql) in [
            (
                "status",
                "ALTER TABLE workflow_runs ADD COLUMN status TEXT NOT NULL DEFAULT 'running'",
            ),
            (
                "workspace_id",
                "ALTER TABLE workflow_runs ADD COLUMN workspace_id TEXT NOT NULL DEFAULT ''",
            ),
            (
                "updated_at_ms",
                "ALTER TABLE workflow_runs ADD COLUMN updated_at_ms INTEGER NOT NULL DEFAULT 0",
            ),
        ] {
            if !columns.iter().any(|c| c == name) {
                connection.execute_batch(sql).map_err(|e| e.to_string())?;
            }
        }
        connection.execute_batch("UPDATE workflow_runs SET status=json_extract(snapshot,'$.status'),workspace_id=json_extract(snapshot,'$.spec.workspace_id'),updated_at_ms=json_extract(snapshot,'$.updated_at_ms') WHERE workspace_id='';
            INSERT INTO workflow_run_summaries(id,status,revision,created_at_ms,updated_at_ms,workspace_id,title,mode)
                SELECT id,json_extract(snapshot,'$.status'),json_extract(snapshot,'$.revision'),json_extract(snapshot,'$.created_at_ms'),
                    json_extract(snapshot,'$.updated_at_ms'),json_extract(snapshot,'$.spec.workspace_id'),json_extract(snapshot,'$.spec.title'),json_extract(snapshot,'$.spec.mode')
                FROM workflow_runs WHERE NOT EXISTS(SELECT 1 FROM workflow_run_summaries s WHERE s.id=workflow_runs.id);
            CREATE INDEX IF NOT EXISTS workflow_run_summaries_workspace_updated ON workflow_run_summaries(workspace_id,updated_at_ms DESC);
            CREATE INDEX IF NOT EXISTS workflow_runs_status ON workflow_runs(status);
            CREATE INDEX IF NOT EXISTS workflow_runs_workspace_updated ON workflow_runs(workspace_id,updated_at_ms DESC);").map_err(|e|e.to_string())?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }
}

pub(super) fn list_summaries(
    connection: &Connection,
    workspace: &str,
    limit: usize,
) -> Result<Vec<RunSummary>, String> {
    let mut statement=connection.prepare("SELECT id,status,revision,created_at_ms,updated_at_ms,workspace_id,title,mode FROM workflow_run_summaries WHERE workspace_id=?1 ORDER BY updated_at_ms DESC,id DESC LIMIT ?2").map_err(|e|e.to_string())?;
    let rows = statement
        .query_map(params![workspace, limit], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, u64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    rows.map(|row| {
        let (id, status, revision, created_at_ms, updated_at_ms, workspace_id, title, mode) =
            row.map_err(|e| e.to_string())?;
        Ok(RunSummary {
            id,
            status: serde_json::from_value(Value::String(status)).map_err(|e| e.to_string())?,
            revision,
            created_at_ms,
            updated_at_ms,
            spec: RunSummarySpec {
                workspace_id,
                title,
                mode: serde_json::from_value(Value::String(mode)).map_err(|e| e.to_string())?,
            },
        })
    })
    .collect()
}

pub(super) fn list_active(connection: &Connection) -> Result<Vec<RunSnapshot>, String> {
    let mut statement=connection.prepare("SELECT snapshot FROM workflow_runs WHERE status IN ('running','paused','unknown','stopping') ORDER BY rowid").map_err(|e|e.to_string())?;
    let rows = statement
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    rows.map(|r| serde_json::from_str(&r.map_err(|e| e.to_string())?).map_err(|e| e.to_string()))
        .collect()
}
pub(super) fn list_recent(
    connection: &Connection,
    workspace: Option<&str>,
    limit: usize,
) -> Result<Vec<RunSnapshot>, String> {
    let mut statement=connection.prepare("SELECT snapshot FROM workflow_runs WHERE (?1 IS NULL OR workspace_id=?1) ORDER BY updated_at_ms DESC,rowid DESC LIMIT ?2").map_err(|e|e.to_string())?;
    let rows = statement
        .query_map(params![workspace, limit], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    rows.map(|r| serde_json::from_str(&r.map_err(|e| e.to_string())?).map_err(|e| e.to_string()))
        .collect()
}

pub(super) fn load(connection: &Connection, id: &str) -> Result<RunSnapshot, String> {
    let value: Option<String> = connection
        .query_row(
            "SELECT snapshot FROM workflow_runs WHERE id=?1",
            [id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&value.ok_or_else(|| format!("Unknown workflow run: {id}"))?)
        .map_err(|e| e.to_string())
}

pub(super) fn save(connection: &Connection, run: &RunSnapshot, kind: &str) -> Result<(), String> {
    let encoded = serde_json::to_string(run).map_err(|e| e.to_string())?;
    let status = serde_json::to_value(run.status).map_err(|e| e.to_string())?;
    let mode = serde_json::to_value(run.spec.mode).map_err(|e| e.to_string())?;
    connection
        .execute(
            "INSERT INTO workflow_runs(id,snapshot,status,workspace_id,updated_at_ms) VALUES(?1,?2,?3,?4,?5)
        ON CONFLICT(id) DO UPDATE SET snapshot=excluded.snapshot,status=excluded.status,workspace_id=excluded.workspace_id,updated_at_ms=excluded.updated_at_ms",
            params![run.id, encoded,status.as_str().ok_or("Invalid run status")?,run.spec.workspace_id,run.updated_at_ms],
        )
        .map_err(|e| e.to_string())?;
    connection.execute("INSERT INTO workflow_run_summaries(id,status,revision,created_at_ms,updated_at_ms,workspace_id,title,mode) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
        ON CONFLICT(id) DO UPDATE SET status=excluded.status,revision=excluded.revision,updated_at_ms=excluded.updated_at_ms,title=excluded.title,mode=excluded.mode",
        params![run.id,status.as_str().ok_or("Invalid run status")?,run.revision,run.created_at_ms,run.updated_at_ms,run.spec.workspace_id,run.spec.title,mode.as_str().ok_or("Invalid run mode")?]).map_err(|e|e.to_string())?;
    connection
        .execute(
            "INSERT INTO workflow_events(run_id,revision,kind,timestamp_ms)
        VALUES(?1,?2,?3,?4)",
            params![run.id, run.revision, kind, run.updated_at_ms],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub(super) fn operation(
    connection: &Connection,
    scope: &str,
    key: &str,
    payload: &Value,
) -> Result<Option<String>, String> {
    let existing: Option<(String, String)> = connection
        .query_row(
            "SELECT payload,run_id FROM workflow_operations WHERE scope=?1 AND key=?2",
            params![scope, key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some((encoded, id)) = existing {
        if serde_json::from_str::<Value>(&encoded).map_err(|e| e.to_string())? != *payload {
            return Err("Idempotency key was already used with a different command".into());
        }
        Ok(Some(id))
    } else {
        Ok(None)
    }
}

pub(super) fn record_operation(
    connection: &Connection,
    scope: &str,
    key: &str,
    payload: &Value,
    run_id: &str,
) -> Result<(), String> {
    let encoded = serde_json::to_string(payload).map_err(|e| e.to_string())?;
    let bytes = encoded.len().saturating_add(key.len());
    let (retained, count): (u64, u64) = connection
        .query_row(
            "SELECT retained_bytes,operation_count FROM workflow_operation_counters WHERE run_id=?1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .unwrap_or((0, 0));
    if count >= 10_000 {
        return Err("Workflow retained operation count limit reached".into());
    }
    if retained.saturating_add(bytes as u64) > 32 * 1024 * 1024 {
        return Err("Workflow retained operation byte budget exceeded".into());
    }
    connection
        .execute(
            "INSERT INTO workflow_operations(scope,key,payload,run_id) VALUES(?1,?2,?3,?4)",
            params![scope, key, encoded, run_id],
        )
        .map_err(|e| e.to_string())?;
    // Both callers own an IMMEDIATE transaction covering graph mutation,
    // durable result, operation identity, and these counters.
    connection.execute("INSERT INTO workflow_operation_counters(run_id,retained_bytes,operation_count) VALUES(?1,?2,1)
        ON CONFLICT(run_id) DO UPDATE SET retained_bytes=retained_bytes+excluded.retained_bytes,operation_count=operation_count+1",
        params![run_id,bytes as u64]).map_err(|e|e.to_string())?;
    Ok(())
}

pub(super) fn reserve_journal(
    connection: &Connection,
    run_id: &str,
    bytes: usize,
    entries: usize,
    max_entries: usize,
) -> Result<(), String> {
    let (retained, count): (u64, u64) = connection
        .query_row(
            "SELECT retained_bytes,entry_count FROM workflow_journal_counters WHERE run_id=?1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .unwrap_or((0, 0));
    if count.saturating_add(entries as u64) > max_entries as u64 {
        return Err("Workflow journal limit reached".into());
    }
    if retained.saturating_add(bytes as u64) > 32 * 1024 * 1024 {
        return Err("Workflow journal byte budget exceeded".into());
    }
    connection.execute("INSERT INTO workflow_journal_counters(run_id,retained_bytes,entry_count) VALUES(?1,?2,?3)
        ON CONFLICT(run_id) DO UPDATE SET retained_bytes=retained_bytes+excluded.retained_bytes,entry_count=entry_count+excluded.entry_count",
        params![run_id,bytes as u64,entries as u64]).map_err(|e|e.to_string())?;
    Ok(())
}

pub(super) fn events(connection: &Connection, after: i64) -> Result<Vec<WorkflowEvent>, String> {
    let mut statement = connection
        .prepare(
            "SELECT sequence,run_id,revision,kind,timestamp_ms
        FROM workflow_events WHERE sequence>?1 ORDER BY sequence LIMIT 1000",
        )
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map([after], |row| {
            Ok(WorkflowEvent {
                sequence: row.get(0)?,
                run_id: row.get(1)?,
                revision: row.get(2)?,
                kind: row.get(3)?,
                timestamp_ms: row.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.map(|row| row.map_err(|e| e.to_string())).collect()
}
