//! Tauri commands for the Automations feature.
//!
//! These wrap the `DatabaseStore` CRUD with frontend-shaped errors and
//! keep the derived `next_run_at` column in step with the `schedule`:
//! every create, schedule edit, or resume recomputes the next fire time
//! from now, so a paused automation never fires a backlog when resumed.
//!
//! Account sync is intentionally not wired here yet — automations carry
//! the same `dirty` flag as hosts, so a future `automations_sync` can
//! push deltas without any change to this surface.

use crate::automations::recurrence;
use crate::database::{AutomationInput, AutomationRecord, AutomationRunRecord, DatabaseStore};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tauri::State;

/// Maximum prompt length. Matches the cap other agentic tools use for a
/// single instruction blob and keeps a stray paste from bloating the DB.
const MAX_PROMPT_LEN: usize = 100_000;
const MAX_NAME_LEN: usize = 200;
const MAX_AGENT_LEN: usize = 50;
const MAX_SCHEDULE_LEN: usize = 2_000;
const MAX_TIMEZONE_LEN: usize = 100;
const MAX_RETENTION: i64 = 1_000;

/// Frontend-facing view of an automation. Drops `deleted_at` (soft-delete
/// tombstones never reach the UI) but keeps `dirty` so a later sync-status
/// indicator has the data it needs.
#[derive(Debug, Serialize, Deserialize)]
pub struct AutomationView {
    pub id: i64,
    pub server_id: Option<String>,
    pub name: String,
    pub prompt: String,
    pub agent: String,
    pub schedule: String,
    pub timezone: String,
    pub host_id: Option<i64>,
    pub project_path: Option<String>,
    pub enabled: bool,
    pub retention_limit: i64,
    pub last_run_at: Option<String>,
    pub next_run_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub dirty: bool,
}

impl From<AutomationRecord> for AutomationView {
    fn from(r: AutomationRecord) -> Self {
        Self {
            id: r.id,
            server_id: r.server_id,
            name: r.name,
            prompt: r.prompt,
            agent: r.agent,
            schedule: r.schedule,
            timezone: r.timezone,
            host_id: r.host_id,
            project_path: r.project_path,
            enabled: r.enabled,
            retention_limit: r.retention_limit,
            last_run_at: r.last_run_at,
            next_run_at: r.next_run_at,
            created_at: r.created_at,
            updated_at: r.updated_at,
            dirty: r.dirty,
        }
    }
}

/// Normalise and validate user input shared by create and update.
/// Returns a cleaned `AutomationInput` (trimmed strings) on success.
fn clean_input(mut input: AutomationInput) -> Result<AutomationInput, String> {
    input.name = input.name.trim().to_string();
    input.agent = input.agent.trim().to_string();
    input.schedule = input.schedule.trim().to_string();
    input.timezone = input.timezone.trim().to_string();
    input.project_path = input
        .project_path
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty());

    if input.name.is_empty() {
        return Err("Automation name cannot be empty".into());
    }
    if input.name.len() > MAX_NAME_LEN {
        return Err(format!("Automation name is too long (max {MAX_NAME_LEN} chars)"));
    }
    if input.prompt.trim().is_empty() {
        return Err("Automation prompt cannot be empty".into());
    }
    if input.prompt.len() > MAX_PROMPT_LEN {
        return Err(format!("Automation prompt is too long (max {MAX_PROMPT_LEN} chars)"));
    }
    if input.agent.is_empty() {
        return Err("An agent must be selected".into());
    }
    if input.agent.len() > MAX_AGENT_LEN {
        return Err("Agent name is too long".into());
    }
    if input.schedule.len() > MAX_SCHEDULE_LEN {
        return Err(format!("Schedule is too long (max {MAX_SCHEDULE_LEN} chars)"));
    }
    if input.timezone.is_empty() {
        return Err("A timezone must be set".into());
    }
    if input.timezone.len() > MAX_TIMEZONE_LEN {
        return Err("Timezone name is too long".into());
    }
    if input.retention_limit < 1 || input.retention_limit > MAX_RETENTION {
        return Err(format!("Retention limit must be between 1 and {MAX_RETENTION}"));
    }
    // The schedule must be a valid RFC 5545 recurrence — reject garbage
    // at the boundary so the host scheduler never has to.
    recurrence::validate(&input.schedule)?;
    Ok(input)
}

/// Recompute and persist `next_run_at` from the current time. The
/// schedule is already validated by `clean_input`, so a parse failure
/// here is unexpected — surface it rather than silently swallowing it.
fn refresh_next_run(db: &DatabaseStore, id: i64, schedule: &str) -> Result<(), String> {
    let next = recurrence::next_occurrence(schedule, Utc::now())?;
    db.set_automation_next_run(id, next.map(|dt| dt.to_rfc3339()).as_deref())
}

// ── Shared implementation ──
//
// The `*_impl` functions take a plain `&DatabaseStore` so both the Tauri
// command surface (desktop UI) and the control socket (agents / MCP)
// run the exact same validation and `next_run_at` bookkeeping. The
// `#[tauri::command]` wrappers below are thin adapters over them.

pub fn list_automations_impl(db: &DatabaseStore) -> Vec<AutomationView> {
    db.list_automations().into_iter().map(Into::into).collect()
}

pub fn get_automation_impl(db: &DatabaseStore, id: i64) -> Result<AutomationView, String> {
    db.get_automation(id)
        .map(Into::into)
        .ok_or_else(|| format!("No automation with id {id}"))
}

pub fn create_automation_impl(
    db: &DatabaseStore,
    input: AutomationInput,
) -> Result<AutomationView, String> {
    let input = clean_input(input)?;
    let record = db.insert_automation(&input)?;
    refresh_next_run(db, record.id, &record.schedule)?;
    db.get_automation(record.id)
        .map(Into::into)
        .ok_or_else(|| "Automation disappeared immediately after creation".to_string())
}

pub fn update_automation_impl(
    db: &DatabaseStore,
    id: i64,
    input: AutomationInput,
) -> Result<AutomationView, String> {
    let input = clean_input(input)?;
    let record = db.update_automation(id, &input)?;
    // The schedule may have changed — recompute the next fire time.
    refresh_next_run(db, id, &record.schedule)?;
    db.get_automation(id)
        .map(Into::into)
        .ok_or_else(|| format!("No automation with id {id}"))
}

/// Pause or resume an automation. Resuming recomputes `next_run_at` from
/// now so the automation never fires the slots it missed while paused.
pub fn set_automation_enabled_impl(
    db: &DatabaseStore,
    id: i64,
    enabled: bool,
) -> Result<AutomationView, String> {
    let record = db.set_automation_enabled(id, enabled)?;
    if enabled {
        refresh_next_run(db, id, &record.schedule)?;
    } else {
        db.set_automation_next_run(id, None)?;
    }
    db.get_automation(id)
        .map(Into::into)
        .ok_or_else(|| format!("No automation with id {id}"))
}

pub fn delete_automation_impl(db: &DatabaseStore, id: i64) -> Result<(), String> {
    db.delete_automation(id)
}

/// Run history for one automation, newest fire first. `limit` is
/// clamped to 1..=100.
pub fn list_automation_runs_impl(
    db: &DatabaseStore,
    automation_id: i64,
    limit: Option<u32>,
) -> Vec<AutomationRunRecord> {
    let limit = limit.unwrap_or(20).clamp(1, 100);
    db.list_automation_runs(automation_id, limit)
}

// ── Tauri command surface ──

#[tauri::command]
pub fn automations_list(db: State<'_, DatabaseStore>) -> Vec<AutomationView> {
    list_automations_impl(db.inner())
}

#[tauri::command]
pub fn automations_get(
    db: State<'_, DatabaseStore>,
    id: i64,
) -> Result<AutomationView, String> {
    get_automation_impl(db.inner(), id)
}

#[tauri::command]
pub fn automations_create(
    db: State<'_, DatabaseStore>,
    input: AutomationInput,
) -> Result<AutomationView, String> {
    create_automation_impl(db.inner(), input)
}

#[tauri::command]
pub fn automations_update(
    db: State<'_, DatabaseStore>,
    id: i64,
    input: AutomationInput,
) -> Result<AutomationView, String> {
    update_automation_impl(db.inner(), id, input)
}

#[tauri::command]
pub fn automations_set_enabled(
    db: State<'_, DatabaseStore>,
    id: i64,
    enabled: bool,
) -> Result<AutomationView, String> {
    set_automation_enabled_impl(db.inner(), id, enabled)
}

#[tauri::command]
pub fn automations_delete(db: State<'_, DatabaseStore>, id: i64) -> Result<(), String> {
    delete_automation_impl(db.inner(), id)
}

#[tauri::command]
pub fn automations_runs(
    db: State<'_, DatabaseStore>,
    automation_id: i64,
    limit: Option<u32>,
) -> Vec<AutomationRunRecord> {
    list_automation_runs_impl(db.inner(), automation_id, limit)
}
