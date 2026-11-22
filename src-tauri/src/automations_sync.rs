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

use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::auth::{api_base_url, is_token_expired, load_token};
use crate::database::automation_triggers::AutomationTrigger;
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
    #[serde(rename = "projectRemote", default)]
    pub project_remote: Option<String>,
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
    #[serde(rename = "projectRemote")]
    project_remote: Option<&'a str>,
    enabled: bool,
    #[serde(rename = "retentionLimit")]
    retention_limit: i64,
}

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

// Keep subsequent passes from settling a close while a POST still has its server ID in flight.
static SYNC_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Pull and push for the current signed-in user, if any.
pub async fn try_sync_with_app<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<(), String> {
    let db = app.state::<DatabaseStore>();
    let _one_at_a_time = SYNC_LOCK.lock().await;
    // A queued pass reads the account that is signed in when it starts.
    let Some(token) = valid_token(&db) else {
        return Ok(());
    };
    sync_locked(&token, &db, true).await
}

async fn sync_locked(
    token: &str,
    db: &DatabaseStore,
    push_after_pull_error: bool,
) -> Result<(), String> {
    let pull = pull_locked(token, db, None).await;
    if !push_after_pull_error {
        pull?;
        return push_locked(token, db).await;
    }
    let pull_err = pull.err();
    let push_err = push_locked(token, db).await.err();
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
    let _one_at_a_time = SYNC_LOCK.lock().await;
    pull_locked(token, db, host_filter).await
}

async fn pull_locked(
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

        // The automation targets a remote host, but that host has not
        // synced to this device yet, so we can't resolve its local id.
        // Importing it now would store `host_id = NULL` — which the
        // desktop scheduler reads as "this machine" and would run a
        // remote-host automation locally. Skip it; a later sync (once
        // the host row arrives) imports it correctly. Tombstones still
        // fall through so a deletion always propagates.
        if a.deleted_at.is_none()
            && a.host_server_id.is_some()
            && host_id.is_none()
        {
            eprintln!(
                "[automations-sync] deferring {}: target host not synced yet",
                a.id
            );
            continue;
        }

        db.upsert_automation_from_server(
            &a.id,
            &a.name,
            &a.prompt,
            &a.agent,
            &a.schedule,
            &a.timezone,
            host_id,
            a.project_path.as_deref(),
            a.project_remote.as_deref(),
            a.enabled,
            a.retention_limit,
            &a.created_at,
            &a.updated_at,
            a.deleted_at.as_deref(),
        )?;
    }

    // Server-side deletion sweep. A row with unpushed changes is left to
    // its push, which settles a row the server dropped (see `push_update`).
    for row in &local {
        if let Some(sid) = &row.server_id {
            if !server_ids.contains(sid) && row.deleted_at.is_none() && !row.dirty {
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
                    row.project_remote.as_deref(),
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
    let _one_at_a_time = SYNC_LOCK.lock().await;
    push_locked(token, db).await
}

async fn push_locked(token: &str, db: &DatabaseStore) -> Result<(), String> {
    let base = api_base_url();
    let client = reqwest::Client::new();
    let (local_to_server, _server_to_local) = host_maps(db);
    let dirty = db.list_dirty_automations();
    let mut any_failed = false;
    // The account's list, fetched at most once a pass, and only if a row
    // turns out to be gone.
    let mut listed: Option<Option<std::collections::HashSet<String>>> = None;

    for row in &dirty {
        // Webhook automations stay on this device: the API has no trigger
        // field, so another device would read one as a broken schedule.
        // Clear `dirty` instead, which also lets a tombstone be purged.
        if row.trigger_type == AutomationTrigger::Webhook {
            if let Err(error) = db.mark_automation_pushed(row, None) {
                eprintln!("[automations-sync] could not settle local id {}: {error}", row.id);
                any_failed = true;
            }
            continue;
        }
        let result = if row.deleted_at.is_some() {
            push_delete(&client, &base, token, row, db).await
        } else if row.server_id.is_none() {
            push_insert(&client, &base, token, row, &local_to_server, db).await
        } else {
            push_update(&client, &base, token, row, &local_to_server, db, &mut listed).await
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
        project_remote: row.project_remote.as_deref(),
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
    db.mark_automation_pushed(row, Some(&parsed.automation.id))?;
    Ok(())
}

async fn push_update(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    row: &AutomationRecord,
    local_to_server: &HashMap<i64, String>,
    db: &DatabaseStore,
    listed: &mut Option<Option<std::collections::HashSet<String>>>,
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
    if resp.status() == reqwest::StatusCode::NOT_FOUND
        && server_lacks_automation(client, base, token, server_id, listed).await
    {
        // A deleted server row wins once the PATCH snapshot is still current.
        return db.mark_automation_missing(row);
    }
    if !resp.status().is_success() {
        return Err(format!("API error: {}", resp.status()));
    }
    db.mark_automation_pushed(row, None)?;
    Ok(())
}

/// Whether the account's automation list is readable and has no live row
/// `server_id`. A server without the automations API answers 404 to
/// everything, so a 404 alone does not say the row is gone. The list is
/// fetched once per push pass, into `listed`; if it cannot be read, no row
/// is taken to be gone.
async fn server_lacks_automation(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    server_id: &str,
    listed: &mut Option<Option<std::collections::HashSet<String>>>,
) -> bool {
    if listed.is_none() {
        let ids = async {
            let resp = client
                .get(format!("{base}/api/automations"))
                .header("Authorization", format!("Bearer {token}"))
                .send()
                .await
                .ok()?;
            if !resp.status().is_success() {
                return None;
            }
            let body = resp.json::<ListAutomationsResponse>().await.ok()?;
            Some(
                body.automations
                    .into_iter()
                    .filter(|a| a.deleted_at.is_none())
                    .map(|a| a.id)
                    .collect(),
            )
        }
        .await;
        *listed = Some(ids);
    }
    matches!(listed, Some(Some(ids)) if !ids.contains(server_id))
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
            return db.mark_automation_pushed(row, None);
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
    db.mark_automation_pushed(row, None)?;
    Ok(())
}

/// Foreground sync with a fixed token. Skip if another pass owns the lock.
pub async fn sync_automations(token: &str, db: &DatabaseStore) -> Result<(), String> {
    let Ok(_one_at_a_time) = SYNC_LOCK.try_lock() else {
        return Ok(());
    };
    sync_locked(token, db, false).await
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
            project_remote: None,
            retention_limit: 10,
            trigger_type: None,
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
    async fn pull_defers_an_automation_whose_target_host_is_not_synced() {
        // The automation targets a remote host, but that host has not
        // synced to this device yet. Importing it would store
        // `host_id = NULL`, which the desktop scheduler reads as "this
        // machine" — it would run a remote-host automation on the
        // wrong machine. Pull must skip it until the host row arrives.
        let mut server = mockito::Server::new_async().await;
        std::env::set_var("CODEMUX_API_URL", server.url());
        let mock = server
            .mock("GET", "/api/automations")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"automations":[{"id":"srv-2","name":"OnHost",
                "prompt":"p","agent":"claude",
                "schedule":"DTSTART:20260101T090000Z\nRRULE:FREQ=DAILY",
                "timezone":"UTC","hostServerId":"host-srv-99","projectPath":null,
                "enabled":true,"retentionLimit":10,
                "createdAt":"2026-01-01T00:00:00Z",
                "updatedAt":"2026-01-01T00:00:00Z","deletedAt":null}]}"#,
            )
            .create_async()
            .await;

        let db = init_test_database();
        pull("token", &db, None).await.unwrap();
        mock.assert_async().await;

        assert!(
            db.list_automations().is_empty(),
            "an automation for an unknown host is deferred, not imported"
        );
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

    #[tokio::test]
    #[serial]
    async fn push_keeps_webhook_automations_on_this_device() {
        let mut server = mockito::Server::new_async().await;
        std::env::set_var("CODEMUX_API_URL", server.url());
        let mock = server
            .mock("POST", "/api/automations")
            .expect(0)
            .create_async()
            .await;

        let db = init_test_database();
        let created = db
            .insert_automation(&AutomationInput {
                schedule: String::new(),
                trigger_type: Some(AutomationTrigger::Webhook),
                ..sample_input()
            })
            .unwrap();
        assert!(created.dirty);

        push("token", &db).await.unwrap();
        mock.assert_async().await;

        let row = db.get_automation(created.id).unwrap();
        assert!(!row.dirty);
        assert!(row.server_id.is_none());
        std::env::remove_var("CODEMUX_API_URL");
    }

    fn server_copy(id: &str, name: &str) -> String {
        format!(
            r#"{{"id":"{id}","name":"{name}","prompt":"p","agent":"claude",
            "schedule":"DTSTART:20260101T090000Z\nRRULE:FREQ=DAILY",
            "timezone":"UTC","hostServerId":null,"projectPath":"/r",
            "enabled":true,"retentionLimit":10,
            "createdAt":"2026-01-01T00:00:00Z",
            "updatedAt":"2026-01-01T00:00:00Z","deletedAt":null}}"#
        )
    }

    #[test]
    fn an_edit_made_while_its_automation_was_being_uploaded_stays_dirty() {
        let db = init_test_database();
        let row = db.insert_automation(&sample_input()).unwrap();
        db.mark_automation_synced(row.id, Some("srv-inflight")).unwrap();
        db.set_automation_enabled(row.id, false).unwrap();

        // The PATCH carrying the pause is in flight when it is deleted.
        let in_flight = db.get_automation(row.id).unwrap();
        db.delete_automation(row.id).unwrap();
        db.mark_automation_pushed(&in_flight, None).unwrap();
        assert!(db.list_dirty_automations()[0].deleted_at.is_some());
        db.mark_automation_missing(&in_flight).unwrap();

        let after = db
            .list_automations_for_sync()
            .into_iter()
            .find(|r| r.id == row.id)
            .unwrap();
        assert!(after.deleted_at.is_some() && after.dirty, "the deletion still goes up");
    }

    #[tokio::test]
    #[serial]
    async fn a_pause_during_first_upload_is_pushed_with_the_receipt_id() {
        let mut server = mockito::Server::new_async().await;
        let db = std::sync::Arc::new(init_test_database());
        let id = db.insert_automation(&sample_input()).unwrap().id;
        let pull = server
            .mock("GET", "/api/automations")
            .match_header("authorization", "Bearer fixed-token")
            .with_status(200)
            .with_body(r#"{"automations":[]}"#)
            .create_async()
            .await;
        let upload = server
            .mock("POST", "/api/automations")
            .with_status(200)
            .with_body_from_request({
                let db = db.clone();
                move |_| {
                    db.set_automation_enabled(id, false).unwrap();
                    format!(r#"{{"automation":{}}}"#, server_copy("srv-new", "First upload"))
                        .into_bytes()
                }
            })
            .create_async()
            .await;
        let patch = server
            .mock("PATCH", "/api/automations/srv-new")
            .match_body(mockito::Matcher::PartialJson(
                serde_json::json!({"enabled": false}),
            ))
            .with_status(204)
            .create_async()
            .await;

        std::env::set_var("CODEMUX_API_URL", server.url());
        let result = async {
            sync_automations("fixed-token", &db).await?;
            let pending = db.list_dirty_automations().pop();
            push("token", &db).await?;
            Ok::<_, String>(pending)
        }
        .await;
        std::env::remove_var("CODEMUX_API_URL");

        let pending = result.unwrap().unwrap();
        assert_eq!(pending.server_id.as_deref(), Some("srv-new"));
        assert!(!pending.enabled);
        assert!(db.list_dirty_automations().is_empty());
        pull.assert_async().await;
        upload.assert_async().await;
        patch.assert_async().await;
    }

    #[tokio::test]
    #[serial]
    async fn pull_keeps_unpushed_edits_pauses_and_deletes() {
        let mut server = mockito::Server::new_async().await;
        std::env::set_var("CODEMUX_API_URL", server.url());
        // The account still has the old, enabled copy of all three.
        let _list = server
            .mock("GET", "/api/automations")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(format!(
                r#"{{"automations":[{},{},{}]}}"#,
                server_copy("srv-1", "Paused"),
                server_copy("srv-2", "Deleted"),
                server_copy("srv-3", "Untouched")
            ))
            .create_async()
            .await;

        let db = init_test_database();
        let mut ids = Vec::new();
        for (name, sid) in [("Paused", "srv-1"), ("Deleted", "srv-2"), ("Old name", "srv-3")] {
            let row = db
                .insert_automation(&AutomationInput { name: name.into(), ..sample_input() })
                .unwrap();
            db.mark_automation_synced(row.id, Some(sid)).unwrap();
            ids.push(row.id);
        }
        db.update_automation(
            ids[0],
            &AutomationInput { name: "Renamed and paused".into(), ..sample_input() },
        )
        .unwrap();
        db.set_automation_enabled(ids[0], false).unwrap();
        db.delete_automation(ids[1]).unwrap();

        let result = pull("token", &db, None).await;
        std::env::remove_var("CODEMUX_API_URL");
        result.unwrap();

        let rows = db.list_automations_for_sync();
        let row = |id: i64| rows.iter().find(|r| r.id == id).unwrap();
        assert_eq!(row(ids[0]).name, "Renamed and paused");
        assert!(!row(ids[0]).enabled && row(ids[0]).dirty, "a pause is not reverted");
        assert!(
            row(ids[1]).deleted_at.is_some() && row(ids[1]).dirty,
            "a deleted automation is not resurrected"
        );
        assert_eq!(row(ids[2]).name, "Untouched", "a clean row still takes the server copy");
        assert_eq!(rows.len(), 3, "no duplicate rows");
    }
}
