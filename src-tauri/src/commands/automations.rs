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
    pub project_remote: Option<String>,
    pub enabled: bool,
    pub retention_limit: i64,
    pub last_run_at: Option<String>,
    pub next_run_at: Option<String>,
    /// Status of the most recent run — drives the at-a-glance health
    /// dot in the list. `None` until the automation has fired.
    pub last_run_status: Option<String>,
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
            project_remote: r.project_remote,
            enabled: r.enabled,
            retention_limit: r.retention_limit,
            last_run_at: r.last_run_at,
            next_run_at: r.next_run_at,
            // Populated by `list_automations_impl`; a bare conversion
            // has no run data.
            last_run_status: None,
            created_at: r.created_at,
            updated_at: r.updated_at,
            dirty: r.dirty,
        }
    }
}

/// Result of probing whether a host can reach an automation's repo.
#[derive(Debug, Serialize)]
pub struct RepoAccessResult {
    pub ok: bool,
    pub message: String,
}

/// Resolve a project's `origin` remote URL — the GitHub backbone a host
/// without `project_path` clones from. `None` for a repo with no
/// `origin` (such an automation can only run on "This machine").
fn resolve_project_remote(project_path: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project_path)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if url.is_empty() {
        None
    } else {
        Some(url)
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
    // Resolve the project's git remote so a remote host can obtain the
    // repo. An explicit caller-supplied value (e.g. from MCP) wins;
    // otherwise derive it from the chosen project.
    input.project_remote = input
        .project_remote
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty())
        .or_else(|| {
            input
                .project_path
                .as_deref()
                .and_then(resolve_project_remote)
        });

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
    db.list_automations()
        .into_iter()
        .map(|record| {
            // Stamp the latest run's status so the list can show a
            // health dot without the frontend fetching per-row history.
            let last_run_status = db
                .list_automation_runs(record.id, 1)
                .into_iter()
                .next()
                .map(|run| run.status);
            let mut view: AutomationView = record.into();
            view.last_run_status = last_run_status;
            view
        })
        .collect()
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

/// Fire-and-forget background sync after a mutation, so the user's
/// other devices pick up the change within seconds. A failed sync is
/// logged, not surfaced — the row stays `dirty` and the next sync
/// retries. Mirrors `commands::hosts::schedule_background_sync`.
pub fn schedule_automations_sync<R: tauri::Runtime>(app: tauri::AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = crate::automations_sync::try_sync_with_app(&app).await {
            eprintln!("[codemux::automations] background sync failed: {error}");
        }
    });
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
pub fn automations_create<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    db: State<'_, DatabaseStore>,
    input: AutomationInput,
) -> Result<AutomationView, String> {
    let view = create_automation_impl(db.inner(), input)?;
    schedule_automations_sync(app);
    Ok(view)
}

#[tauri::command]
pub fn automations_update<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    db: State<'_, DatabaseStore>,
    id: i64,
    input: AutomationInput,
) -> Result<AutomationView, String> {
    let view = update_automation_impl(db.inner(), id, input)?;
    schedule_automations_sync(app);
    Ok(view)
}

#[tauri::command]
pub fn automations_set_enabled<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    db: State<'_, DatabaseStore>,
    id: i64,
    enabled: bool,
) -> Result<AutomationView, String> {
    let view = set_automation_enabled_impl(db.inner(), id, enabled)?;
    schedule_automations_sync(app);
    Ok(view)
}

#[tauri::command]
pub fn automations_delete<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    db: State<'_, DatabaseStore>,
    id: i64,
) -> Result<(), String> {
    delete_automation_impl(db.inner(), id)?;
    schedule_automations_sync(app);
    Ok(())
}

#[tauri::command]
pub fn automations_runs(
    db: State<'_, DatabaseStore>,
    automation_id: i64,
    limit: Option<u32>,
) -> Vec<AutomationRunRecord> {
    list_automation_runs_impl(db.inner(), automation_id, limit)
}

// ── Repo-access preflight ──
//
// `git ls-remote` does a real authentication handshake against the
// exact repo URL without cloning — the canonical "can this host reach
// this repository" probe. It is read-only and remote-agnostic (works
// for GitHub, GitLab, a private server).

/// Marker the host-side probe echoes on a successful `git ls-remote`.
const REPO_PROBE_MARKER: &str = "CODEMUX_LSREMOTE_OK";

/// Whether the host-side probe output indicates success. Pure.
fn interpret_repo_probe(stdout: &str) -> bool {
    stdout.contains(REPO_PROBE_MARKER)
}

/// Check whether the automation's host can reach its project repo.
///
/// "This machine" always passes — it uses the local project directly.
/// For a remote host this SSHes in and runs `git ls-remote`; a failure
/// is reported (never thrown) so the form can warn without blocking.
///
/// The repo is identified by `project_remote` when known (the detail
/// view has it), else resolved from `project_path` (the create form
/// only has the path).
#[tauri::command]
pub async fn automations_check_repo_access(
    db: State<'_, DatabaseStore>,
    host_id: Option<i64>,
    project_path: Option<String>,
    project_remote: Option<String>,
) -> Result<RepoAccessResult, String> {
    let host_id = match host_id {
        Some(id) => id,
        None => {
            return Ok(RepoAccessResult {
                ok: true,
                message: "Runs on this machine — the project is used directly."
                    .to_string(),
            })
        }
    };
    let resolved = project_remote
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty())
        .or_else(|| {
            project_path
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .and_then(resolve_project_remote)
        });
    let remote = match resolved {
        Some(r) => r,
        None => {
            return Ok(RepoAccessResult {
                ok: false,
                message: "This project has no git remote, so it can only run \
                          on this machine."
                    .to_string(),
            })
        }
    };
    // The remote URL is interpolated into a single-quoted token in the
    // host-side command; a `'` would break out of the quoting. No git
    // URL contains one, so reject it rather than risk an injection.
    if remote.contains('\'') {
        return Ok(RepoAccessResult {
            ok: false,
            message: "The project's git remote URL contains an unexpected \
                      character."
                .to_string(),
        });
    }
    let host = db
        .list_hosts()
        .into_iter()
        .find(|h| h.id == host_id)
        .ok_or_else(|| format!("Host not found: {host_id}"))?;

    let probe = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        tokio::process::Command::new("ssh")
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("ConnectTimeout=10")
            .arg(&host.ssh_target)
            .arg(format!(
                "GIT_TERMINAL_PROMPT=0 git ls-remote --heads '{remote}' \
                 >/dev/null 2>&1 && echo {REPO_PROBE_MARKER}"
            ))
            .output(),
    )
    .await;

    match probe {
        Ok(Ok(output)) if interpret_repo_probe(&String::from_utf8_lossy(&output.stdout)) => {
            Ok(RepoAccessResult {
                ok: true,
                message: format!("{} can reach this repository.", host.name),
            })
        }
        Ok(Ok(_)) => Ok(RepoAccessResult {
            ok: false,
            message: format!(
                "{} can't reach this repository. Give it access — an SSH \
                 deploy key, or a `gh auth login` with access to the repo — \
                 then check again. You can still create the automation.",
                host.name
            ),
        }),
        Ok(Err(error)) => Ok(RepoAccessResult {
            ok: false,
            message: format!("Couldn't run the check: {error}"),
        }),
        Err(_) => Ok(RepoAccessResult {
            ok: false,
            message: format!("Timed out reaching {}.", host.name),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::interpret_repo_probe;

    #[test]
    fn repo_probe_succeeds_when_the_marker_is_present() {
        assert!(interpret_repo_probe("CODEMUX_LSREMOTE_OK\n"));
    }

    #[test]
    fn repo_probe_fails_on_empty_or_unmarked_output() {
        assert!(!interpret_repo_probe(""));
        assert!(!interpret_repo_probe("fatal: Authentication failed\n"));
    }
}
