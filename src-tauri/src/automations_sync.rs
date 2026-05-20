//! Automations sync — pull/push the user's automation registry across
//! their devices. A near-copy of `hosts_sync.rs`.
//!
//! Only the `automations` table syncs; `automation_runs` are per-device
//! history and stay local.
//!
//! Host identity crosses the wire as the host's `server_id`
//! (`hostServerId`), not the local integer `host_id` — local ids differ
//! per device. On pull, an unknown `hostServerId` resolves to a null
//! `host_id` (shown as a removed/unknown host until the host syncs in).
//!
//! Failure policy matches `hosts_sync`: a failed push leaves the row
//! `dirty` and logs once; the next sync retries. `404` from the API is
//! treated as "endpoint not deployed yet" — a harmless skip.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::auth::{api_base_url, is_token_expired, load_token};
use crate::database::{AutomationRecord, DatabaseStore};

/// Wire shape from `GET /api/automations` and `POST /api/automations`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerAutomation {
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub agent: String,
    pub schedule: String,
    pub timezone: String,
    #[serde(rename = "hostServerId")]
    pub host_server_id: Option<String>,
    #[serde(rename = "projectPath")]
    pub project_path: Option<String>,
    pub enabled: bool,
    #[serde(rename = "retentionLimit")]
    pub retention_limit: i64,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    #[serde(rename = "deletedAt")]
    pub deleted_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ListAutomationsResponse {
    automations: Vec<ServerAutomation>,
}

#[derive(Debug, Deserialize)]
struct OneAutomationResponse {
    automation: ServerAutomation,
}

#[derive(Debug, Serialize)]
struct AutomationUpsertBody<'a> {
    name: &'a str,
    prompt: &'a str,
    agent: &'a str,
    schedule: &'a str,
    timezone: &'a str,
    #[serde(rename = "hostServerId")]
    host_server_id: Option<String>,
    #[serde(rename = "projectPath")]
    project_path: Option<&'a str>,
    enabled: bool,
    #[serde(rename = "retentionLimit")]
    retention_limit: i64,
}

/// Guards against overlapping sync runs (foreground sync vs. the
/// fire-and-forget sync each mutation triggers).
static SYNC_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Build the two host-identity maps: local `host_id` → `server_id`, and
/// `server_id` → local `host_id`. Only hosts that have synced (have a
/// `server_id`) appear.
fn host_maps(db: &DatabaseStore) -> (HashMap<i64, String>, HashMap<String, i64>) {
    let mut local_to_server = HashMap::new();
    let mut server_to_local = HashMap::new();
    for host in db.list_hosts_for_sync() {
        if let Some(sid) = host.server_id {
            local_to_server.insert(host.id, sid.clone());
            server_to_local.insert(sid, host.id);
        }
    }
    (local_to_server, server_to_local)
}

/// Resolve the local DB + token from app state and run pull-then-push.
/// `Ok(())` when the user is not signed in — sync is not an error then.
pub async fn try_sync_with_app(app: &tauri::AppHandle) -> Result<(), String> {
    let db = app.state::<DatabaseStore>();
    let token = match valid_token(&db) {
        Some(t) => t,
        None => return Ok(()),
    };
    let db_ref: &DatabaseStore = &db;
    let pull_err = pull(&token, db_ref, None).await.err();
    let push_err = push(&token, db_ref).await.err();
    match (pull_err, push_err) {
        (None, None) => Ok(()),
        (Some(p), None) => Err(format!("pull failed: {p}")),
        (None, Some(p)) => Err(format!("push failed: {p}")),
        (Some(a), Some(b)) => Err(format!("pull failed: {a}; push failed: {b}")),
    }
}

fn valid_token(db: &DatabaseStore) -> Option<String> {
    let (token, expires_at) = load_token(db)?;
    if is_token_expired(&expires_at) {
        None
    } else {
        Some(token)
    }
}

/// Pull server automations into the local DB. Idempotent. A local row
/// whose `server_id` is missing from the response was deleted on
/// another device and is tombstoned locally.
///
/// `host_filter` scopes the pull to one host's automations (by host
/// `server_id`): the desktop passes `None` and pulls the whole
/// registry; a `codemux-remote scheduler` passes its own host id so it
/// only receives — and therefore only runs — automations routed to it.
pub async fn pull(
    token: &str,
    db: &DatabaseStore,
    host_filter: Option<&str>,
) -> Result<(), String> {
    let base = api_base_url();
    let client = reqwest::Client::new();
    let mut url = format!("{base}/api/automations");
    if let Some(host) = host_filter {
        url.push_str(&format!("?hostServerId={}", urlencoding::encode(host)));
    }
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    if !resp.status().is_success() {
        // 404 = endpoint not deployed yet — harmless skip.
        if resp.status().as_u16() == 404 {
            return Ok(());
        }
        return Err(format!("API error: {}", resp.status()));
    }
    let body: ListAutomationsResponse =
        resp.json().await.map_err(|e| format!("Parse: {e}"))?;

    let (_local_to_server, server_to_local) = host_maps(db);
    let local = db.list_automations_for_sync();
    let server_ids: std::collections::HashSet<String> =
        body.automations.iter().map(|a| a.id.clone()).collect();

    for a in &body.automations {
        let host_id = a
            .host_server_id
            .as_ref()
            .and_then(|sid| server_to_local.get(sid).copied());
        db.upsert_automation_from_server(
            &a.id,
            &a.name,
            &a.prompt,
            &a.agent,
            &a.schedule,
            &a.timezone,
            host_id,
            a.project_path.as_deref(),
            a.enabled,
            a.retention_limit,
            &a.created_at,
            &a.updated_at,
            a.deleted_at.as_deref(),
        )?;
    }

    // Server-side deletion sweep.
    for row in &local {
        if let Some(sid) = &row.server_id {
            if !server_ids.contains(sid) && row.deleted_at.is_none() {
                eprintln!(
                    "[automations-sync] server no longer has {sid}; tombstoning locally"
                );
                let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
                db.upsert_automation_from_server(
                    sid,
                    &row.name,
                    &row.prompt,
                    &row.agent,
                    &row.schedule,
                    &row.timezone,
                    row.host_id,
                    row.project_path.as_deref(),
                    row.enabled,
                    row.retention_limit,
                    &row.created_at,
                    &now,
                    Some(&now),
                )?;
            }
        }
    }

    Ok(())
}

/// Push dirty local automations. Each row is handled independently so
/// one failure does not strand the rest.
pub async fn push(token: &str, db: &DatabaseStore) -> Result<(), String> {
    let base = api_base_url();
    let client = reqwest::Client::new();
    let (local_to_server, _server_to_local) = host_maps(db);
    let dirty = db.list_dirty_automations();
    let mut any_failed = false;

    for row in &dirty {
        let result = if row.deleted_at.is_some() {
            push_delete(&client, &base, token, row, db).await
        } else if row.server_id.is_none() {
            push_insert(&client, &base, token, row, &local_to_server, db).await
        } else {
            push_update(&client, &base, token, row, &local_to_server, db).await
        };
        if let Err(error) = result {
            eprintln!("[automations-sync] push failed for local id {}: {error}", row.id);
            any_failed = true;
        }
    }

    db.purge_acknowledged_automation_deletes()?;

    if any_failed {
        Err("one or more automation pushes failed; see logs".into())
    } else {
        Ok(())
    }
}

fn upsert_body<'a>(
    row: &'a AutomationRecord,
    local_to_server: &HashMap<i64, String>,
) -> AutomationUpsertBody<'a> {
    AutomationUpsertBody {
        name: &row.name,
        prompt: &row.prompt,
        agent: &row.agent,
        schedule: &row.schedule,
        timezone: &row.timezone,
        host_server_id: row.host_id.and_then(|id| local_to_server.get(&id).cloned()),
        project_path: row.project_path.as_deref(),
        enabled: row.enabled,
        retention_limit: row.retention_limit,
    }
}

async fn push_insert(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    row: &AutomationRecord,
    local_to_server: &HashMap<i64, String>,
    db: &DatabaseStore,
) -> Result<(), String> {
    let resp = client
        .post(format!("{base}/api/automations"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&upsert_body(row, local_to_server))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("API error: {}", resp.status()));
    }
    let parsed: OneAutomationResponse =
        resp.json().await.map_err(|e| format!("Parse: {e}"))?;
    db.mark_automation_synced(row.id, Some(&parsed.automation.id))?;
    Ok(())
}

async fn push_update(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    row: &AutomationRecord,
    local_to_server: &HashMap<i64, String>,
    db: &DatabaseStore,
) -> Result<(), String> {
    let server_id = row
        .server_id
        .as_ref()
        .ok_or_else(|| "push_update called without server_id".to_string())?;
    let resp = client
        .patch(format!("{base}/api/automations/{server_id}"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&upsert_body(row, local_to_server))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("API error: {}", resp.status()));
    }
    db.mark_automation_synced(row.id, None)?;
    Ok(())
}

async fn push_delete(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    row: &AutomationRecord,
    db: &DatabaseStore,
) -> Result<(), String> {
    let server_id = match &row.server_id {
        Some(sid) => sid,
        None => {
            // Created and deleted before it ever synced — nothing on
            // the server. Clear `dirty` so the tombstone is purged.
            db.mark_automation_synced(row.id, None)?;
            return Ok(());
        }
    };
    let resp = client
        .delete(format!("{base}/api/automations/{server_id}"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    // 404 = already gone server-side — also a success for our purposes.
    if !resp.status().is_success() && resp.status().as_u16() != 404 {
        return Err(format!("API error: {}", resp.status()));
    }
    // Clear `dirty` so `purge_acknowledged_automation_deletes` removes
    // the tombstone on the next pass.
    db.mark_automation_synced(row.id, None)?;
    Ok(())
}

/// Foreground sync with the in-progress guard. Used at scheduler
/// startup and by the periodic pull in the tick loop.
pub async fn sync_automations(token: &str, db: &DatabaseStore) -> Result<(), String> {
    if SYNC_IN_PROGRESS.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let result = async {
        pull(token, db, None).await?;
        push(token, db).await?;
        Ok(())
    }
    .await;
    SYNC_IN_PROGRESS.store(false, Ordering::SeqCst);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{init_test_database, AutomationInput};
    use serial_test::serial;

    fn sample_input() -> AutomationInput {
        AutomationInput {
            name: "Triage".to_string(),
            prompt: "review issues".to_string(),
            agent: "claude".to_string(),
            schedule: "DTSTART:20260101T090000Z\nRRULE:FREQ=DAILY".to_string(),
            timezone: "UTC".to_string(),
            host_id: None,
            project_path: Some("/repo".to_string()),
            retention_limit: 10,
        }
    }

    #[tokio::test]
    #[serial]
    async fn pull_upserts_server_automations() {
        let mut server = mockito::Server::new_async().await;
        std::env::set_var("CODEMUX_API_URL", server.url());
        let mock = server
            .mock("GET", "/api/automations")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"automations":[{"id":"srv-1","name":"Nightly",
                "prompt":"p","agent":"claude",
                "schedule":"DTSTART:20260101T090000Z\nRRULE:FREQ=DAILY",
                "timezone":"UTC","hostServerId":null,"projectPath":"/r",
                "enabled":true,"retentionLimit":10,
                "createdAt":"2026-01-01T00:00:00Z",
                "updatedAt":"2026-01-01T00:00:00Z","deletedAt":null}]}"#,
            )
            .create_async()
            .await;

        let db = init_test_database();
        pull("token", &db, None).await.unwrap();
        mock.assert_async().await;

        let rows = db.list_automations();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Nightly");
        assert_eq!(rows[0].server_id.as_deref(), Some("srv-1"));
        assert!(!rows[0].dirty, "server-sourced rows are clean");
        std::env::remove_var("CODEMUX_API_URL");
    }

    #[tokio::test]
    #[serial]
    async fn pull_treats_404_as_a_harmless_skip() {
        let mut server = mockito::Server::new_async().await;
        std::env::set_var("CODEMUX_API_URL", server.url());
        server
            .mock("GET", "/api/automations")
            .with_status(404)
            .create_async()
            .await;
        let db = init_test_database();
        assert!(pull("token", &db, None).await.is_ok());
        std::env::remove_var("CODEMUX_API_URL");
    }

    #[tokio::test]
    #[serial]
    async fn push_uploads_a_dirty_row_and_marks_it_synced() {
        let mut server = mockito::Server::new_async().await;
        std::env::set_var("CODEMUX_API_URL", server.url());
        let mock = server
            .mock("POST", "/api/automations")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"automation":{"id":"srv-9","name":"Triage","prompt":"p",
                "agent":"claude","schedule":"s","timezone":"UTC",
                "hostServerId":null,"projectPath":null,"enabled":true,
                "retentionLimit":10,"createdAt":"x","updatedAt":"x",
                "deletedAt":null}}"#,
            )
            .create_async()
            .await;

        let db = init_test_database();
        let created = db.insert_automation(&sample_input()).unwrap();
        assert!(created.dirty);

        push("token", &db).await.unwrap();
        mock.assert_async().await;

        let row = db.get_automation(created.id).unwrap();
        assert!(!row.dirty, "a pushed row is no longer dirty");
        assert_eq!(row.server_id.as_deref(), Some("srv-9"));
        std::env::remove_var("CODEMUX_API_URL");
    }
}
