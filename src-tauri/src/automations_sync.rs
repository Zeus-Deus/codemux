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

use std::collections::{HashMap, HashSet};

use crate::sync_guard::{
    combine, order_by_last_attempt, record_attempt, signed_in_token, Attempts, CreateReceipt,
    InFlightCreates, SyncGuard, SyncRequest,
};
use futures_util::future::join_all;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::auth::AuthState;
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
    automation: CreateReceipt,
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

// A first upload waits for its receipt outside the pull and known-ID lock.
static SYNC_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static IN_FLIGHT_CREATES: std::sync::LazyLock<InFlightCreates> =
    std::sync::LazyLock::new(InFlightCreates::default);
static ATTEMPTS: Attempts = std::sync::LazyLock::new(Default::default);

static BACKGROUND: crate::sync_guard::Coalescer = crate::sync_guard::Coalescer::new();

pub(crate) fn schedule_background_sync<R: tauri::Runtime>(app: tauri::AppHandle<R>) {
    if !BACKGROUND.request() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        loop {
            if let Err(error) = try_sync_with_app(&app).await {
                eprintln!("[automations-sync] background sync failed: {error}");
            }
            if !BACKGROUND.finished_pass() {
                break;
            }
        }
    });
}

/// Pull and push known IDs for the signed-in user, and start first uploads
/// separately so the desktop can keep syncing while a receipt is pending.
pub async fn try_sync_with_app<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<(), String> {
    let linked = SYNC_LOCK.lock().await;
    let Some(auth) = app.try_state::<AuthState>() else {
        return Ok(());
    };
    let db = app.state::<DatabaseStore>();
    let (generation, token) = signed_in_token(&auth, &db);
    let Some(token) = token else {
        return Ok(());
    };
    let guard = SyncGuard::session(&auth, generation);
    let request = SyncRequest::linked(&token, guard);
    let pull = pull_locked(&request, &db, None).await;
    let push = push_linked(&request, &db).await;
    drop(linked);

    let ids = pending_creates(&request, &db)?;
    if !ids.is_empty() {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let auth = app.state::<AuthState>();
            let db = app.state::<DatabaseStore>();
            let request = SyncRequest::new(&token, SyncGuard::session(&auth, generation));
            if let Err(error) = push_creates(&request, &db, ids, true).await {
                eprintln!("[automations-sync] first uploads failed: {error}");
            }
        });
    }
    combine(pull, push)
}

fn pending_creates(request: &SyncRequest<'_>, db: &DatabaseStore) -> Result<Vec<i64>, String> {
    request.commit(|| {
        db.list_dirty_automations()
            .into_iter()
            .filter(|row| needs_create(row) && !IN_FLIGHT_CREATES.contains(row.id))
            .map(|row| row.id)
            .collect()
    })
}

fn needs_create(row: &AutomationRecord) -> bool {
    row.server_id.is_none()
        && row.deleted_at.is_none()
        && row.trigger_type != AutomationTrigger::Webhook
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
    let _linked = SYNC_LOCK.lock().await;
    let request = SyncRequest::linked(token, SyncGuard::unchecked());
    pull_locked(&request, db, host_filter).await
}

async fn pull_locked(
    request: &SyncRequest<'_>,
    db: &DatabaseStore,
    host_filter: Option<&str>,
) -> Result<(), String> {
    let mut path = "/api/automations".to_string();
    if let Some(host) = host_filter {
        path.push_str(&format!("?hostServerId={}", urlencoding::encode(host)));
    }
    let resp = request.send(request.request(Method::GET, &path)).await?;
    if !resp.status().is_success() {
        // 404 = endpoint not deployed yet — harmless skip.
        if resp.status().as_u16() == 404 {
            return Ok(());
        }
        return Err(format!("API error: {}", resp.status()));
    }
    let body: ListAutomationsResponse =
        resp.json().await.map_err(|e| format!("Parse: {e}"))?;

    request.commit(|| apply_pulled(db, &body))?
}

fn apply_pulled(db: &DatabaseStore, body: &ListAutomationsResponse) -> Result<(), String> {
    let (_local_to_server, server_to_local) = host_maps(db);
    let local = db.list_automations_for_sync();
    let server_ids: HashSet<String> = body.automations.iter().map(|a| a.id.clone()).collect();

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
    let linked = SYNC_LOCK.lock().await;
    sync_fixed(token, db, linked, false).await
}

async fn sync_fixed(
    token: &str,
    db: &DatabaseStore,
    linked_lock: tokio::sync::MutexGuard<'_, ()>,
    pull_first: bool,
) -> Result<(), String> {
    let request = SyncRequest::linked(token, SyncGuard::unchecked());
    if pull_first {
        pull_locked(&request, db, None).await?;
    }
    let linked = push_linked(&request, db).await;
    drop(linked_lock);
    let request = SyncRequest::new(token, SyncGuard::unchecked());
    let creates = push_creates(&request, db, pending_creates(&request, db)?, false).await;
    linked.and(creates)
}

async fn push_linked(request: &SyncRequest<'_>, db: &DatabaseStore) -> Result<(), String> {
    let (local_to_server, mut dirty) =
        request.commit(|| (host_maps(db).0, db.list_dirty_automations()))?;
    dirty.retain(|row| {
        row.server_id.is_some() || (!needs_create(row) && !IN_FLIGHT_CREATES.contains(row.id))
    });
    order_by_last_attempt(&ATTEMPTS, &mut dirty, |row| row.id);
    let mut any_failed = false;
    // The account's list, fetched at most once a pass, and only if a row
    // turns out to be gone.
    let mut listed: Option<Option<HashSet<String>>> = None;

    for row in &dirty {
        if request.expired() {
            any_failed = true;
            break;
        }
        record_attempt(&ATTEMPTS, row.id);
        // Webhook automations stay on this device; the API has no trigger field.
        let result = if row.trigger_type == AutomationTrigger::Webhook {
            request.commit(|| db.mark_automation_pushed(row, None))?
        } else if row.deleted_at.is_some() {
            push_delete(request, row, db).await
        } else {
            push_update(request, row, &local_to_server, db, &mut listed).await
        };
        if let Err(error) = result {
            eprintln!("[automations-sync] push failed for local id {}: {error}", row.id);
            any_failed = true;
        }
    }

    request.commit(|| db.purge_acknowledged_automation_deletes())??;

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

async fn push_creates(
    request: &SyncRequest<'_>,
    db: &DatabaseStore,
    ids: Vec<i64>,
    follow_up: bool,
) -> Result<(), String> {
    let results = join_all(ids.into_iter().map(|id| async move {
        let result = push_create(request, db, id, follow_up).await;
        if let Err(error) = &result {
            eprintln!("[automations-sync] first upload failed for local id {id}: {error}");
        }
        result
    }))
    .await;
    results.into_iter().collect()
}

async fn push_create(
    request: &SyncRequest<'_>,
    db: &DatabaseStore,
    id: i64,
    follow_up: bool,
) -> Result<(), String> {
    let Some(permit) = IN_FLIGHT_CREATES.try_start(id) else {
        return Ok(());
    };
    let (row, local_to_server) =
        request.commit(|| (db.get_dirty_automation(id), host_maps(db).0))?;
    let Some(row) = row.filter(needs_create) else {
        return Ok(());
    };
    let resp = request
        .send(
            request
                .request(Method::POST, "/api/automations")
                .json(&upsert_body(&row, &local_to_server)),
        )
        .await?;
    if !resp.status().is_success() {
        return Err(format!("API error: {}", resp.status()));
    }
    let parsed: OneAutomationResponse =
        resp.json().await.map_err(|e| format!("Parse: {e}"))?;
    let server_id = &parsed.automation.id;
    if server_id.trim().is_empty() {
        return Err("First upload receipt has no server ID".into());
    }
    // Serialize binding against pull so a GET begun before this upload
    // was linked cannot apply to its newly known ID.
    let _linked = SYNC_LOCK.lock().await;
    request.commit(|| db.mark_automation_pushed(&row, Some(server_id)))??;
    drop(permit);

    // A delete or edit made during the POST may have already triggered a sync.
    if follow_up {
        if let Some(pending) = request.commit(|| db.get_dirty_automation(id))? {
            if pending.deleted_at.is_some() {
                push_delete(request, &pending, db).await?;
                request.commit(|| db.purge_acknowledged_automation_deletes())??;
            } else {
                let local_to_server = request.commit(|| host_maps(db).0)?;
                push_update(request, &pending, &local_to_server, db, &mut None).await?;
            }
        }
    }
    Ok(())
}

async fn push_update(
    request: &SyncRequest<'_>,
    row: &AutomationRecord,
    local_to_server: &HashMap<i64, String>,
    db: &DatabaseStore,
    listed: &mut Option<Option<HashSet<String>>>,
) -> Result<(), String> {
    let server_id = row
        .server_id
        .as_ref()
        .ok_or_else(|| "push_update called without server_id".to_string())?;
    let resp = request
        .send(
            request
                .request(Method::PATCH, &format!("/api/automations/{server_id}"))
                .json(&upsert_body(row, local_to_server)),
        )
        .await?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND
        && server_lacks_automation(request, server_id, listed).await
    {
        // A deleted server row wins once the PATCH snapshot is still current.
        return request.commit(|| db.mark_automation_missing(row))?;
    }
    if !resp.status().is_success() {
        return Err(format!("API error: {}", resp.status()));
    }
    request.commit(|| db.mark_automation_pushed(row, None))?
}

/// Whether the account's automation list is readable and has no live row
/// `server_id`. A server without the automations API answers 404 to
/// everything, so a 404 alone does not say the row is gone. The list is
/// fetched once per push pass, into `listed`; if it cannot be read, no row
/// is taken to be gone.
async fn server_lacks_automation(
    request: &SyncRequest<'_>,
    server_id: &str,
    listed: &mut Option<Option<HashSet<String>>>,
) -> bool {
    if listed.is_none() {
        let ids = async {
            let resp = request
                .send(request.request(Method::GET, "/api/automations"))
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
    request: &SyncRequest<'_>,
    row: &AutomationRecord,
    db: &DatabaseStore,
) -> Result<(), String> {
    let server_id = match &row.server_id {
        Some(sid) => sid,
        None => {
            // Created and deleted before it ever synced — nothing on
            // the server. Clear `dirty` so the tombstone is purged.
            return request.commit(|| db.mark_automation_pushed(row, None))?;
        }
    };
    let resp = request
        .send(request.request(Method::DELETE, &format!("/api/automations/{server_id}")))
        .await?;
    // 404 = already gone server-side — also a success for our purposes.
    if !resp.status().is_success() && resp.status().as_u16() != 404 {
        return Err(format!("API error: {}", resp.status()));
    }
    // Clear `dirty` so `purge_acknowledged_automation_deletes` removes
    // the tombstone on the next pass.
    request.commit(|| db.mark_automation_pushed(row, None))?
}

/// Foreground sync with a fixed token. Skip if another pass owns the lock.
pub async fn sync_automations(token: &str, db: &DatabaseStore) -> Result<(), String> {
    let Ok(linked) = SYNC_LOCK.try_lock() else {
        return Ok(());
    };
    sync_fixed(token, db, linked, true).await
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
    async fn first_upload_rereads_queued_rows_before_dispatch() {
        let db = init_test_database();
        let deleted = db.insert_automation(&sample_input()).unwrap();
        let paused = db
            .insert_automation(&AutomationInput {
                name: "Paused before dispatch".into(),
                ..sample_input()
            })
            .unwrap();
        let mut server = mockito::Server::new_async().await;
        let upload = server
            .mock("POST", "/api/automations")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "name": "Paused before dispatch",
                "enabled": false,
            })))
            .with_body(r#"{"automation":{"id":"srv-paused"}}"#)
            .create_async()
            .await;
        let stale_upload = server
            .mock("POST", "/api/automations")
            .match_body(mockito::Matcher::PartialJson(
                serde_json::json!({"enabled": true}),
            ))
            .expect(0)
            .create_async()
            .await;

        std::env::set_var("CODEMUX_API_URL", server.url());
        let request = SyncRequest::new("fixed-token", SyncGuard::unchecked());
        let ids = pending_creates(&request, &db).unwrap();
        assert_eq!(ids.len(), 2);
        db.delete_automation(deleted.id).unwrap();
        db.set_automation_enabled(paused.id, false).unwrap();
        let result = push_creates(&request, &db, ids, false).await;
        std::env::remove_var("CODEMUX_API_URL");

        result.unwrap();
        upload.assert_async().await;
        stale_upload.assert_async().await;
        let row = db.get_automation(paused.id).unwrap();
        assert_eq!(row.server_id.as_deref(), Some("srv-paused"));
        assert!(!row.dirty && !row.enabled);
    }

    #[tokio::test]
    #[serial]
    async fn background_sync_continues_while_a_first_upload_is_stalled() {
        use axum::{http::StatusCode, routing, Json, Router};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::{sync::Arc, time::Duration};
        use tokio::sync::Notify;

        let upload_started = Arc::new(Notify::new());
        let release_receipt = Arc::new(Notify::new());
        let linked_deleted = Arc::new(Notify::new());
        let receipt_deleted = Arc::new(Notify::new());
        let pulls = Arc::new(AtomicUsize::new(0));
        let routes = Router::new()
            .route(
                "/api/automations",
                routing::get(move || {
                    let count = pulls.fetch_add(1, Ordering::SeqCst);
                    async move {
                        let mut rows = vec![server_copy("srv-linked", "Linked")];
                        if count > 0 {
                            rows.push(server_copy("srv-imported", "Imported"));
                        }
                        format!(r#"{{"automations":[{}]}}"#, rows.join(","))
                    }
                })
                .post({
                    let started = upload_started.clone();
                    let release = release_receipt.clone();
                    move |Json(body): Json<serde_json::Value>| async move {
                        let id = match body["name"].as_str().unwrap() {
                            "Triage" => {
                                started.notify_one();
                                release.notified().await;
                                "srv-receipt"
                            }
                            "Queued later" => "srv-queued",
                            name => panic!("unexpected upload: {name}"),
                        };
                        Json(serde_json::json!({"automation": {"id": id}}))
                    }
                }),
            )
            .route(
                "/api/automations/srv-linked",
                routing::delete({
                    let deleted = linked_deleted.clone();
                    move || async move {
                        deleted.notify_one();
                        StatusCode::NO_CONTENT
                    }
                }),
            )
            .route(
                "/api/automations/srv-receipt",
                routing::delete({
                    let deleted = receipt_deleted.clone();
                    move || async move {
                        deleted.notify_one();
                        StatusCode::NO_CONTENT
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, routes).await.unwrap() });

        let app = tauri::test::mock_app();
        app.manage(init_test_database());
        app.manage(AuthState::default());
        let db = app.state::<DatabaseStore>();
        crate::auth::save_token(&db, "desktop-token", "2099-01-01T00:00:00Z").unwrap();
        let linked = db.insert_automation(&sample_input()).unwrap();
        db.mark_automation_synced(linked.id, Some("srv-linked"))
            .unwrap();
        let first = db.insert_automation(&sample_input()).unwrap();

        std::env::set_var("CODEMUX_API_URL", url);
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            schedule_background_sync(app.handle().clone());
            upload_started.notified().await;

            db.delete_automation(linked.id).unwrap();
            db.delete_automation(first.id).unwrap();
            let queued = db
                .insert_automation(&AutomationInput {
                    name: "Queued later".into(),
                    ..sample_input()
                })
                .unwrap();
            schedule_background_sync(app.handle().clone());
            linked_deleted.notified().await;

            let pending = db
                .list_dirty_automations()
                .into_iter()
                .find(|row| row.id == first.id)
                .unwrap();
            assert!(pending.deleted_at.is_some() && pending.server_id.is_none());
            assert!(db
                .list_automations()
                .iter()
                .any(|row| row.server_id.as_deref() == Some("srv-imported")));
            while db.get_automation(queued.id).unwrap().server_id.is_none() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(
                db.get_automation(queued.id).unwrap().server_id.as_deref(),
                Some("srv-queued")
            );
            release_receipt.notify_one();
            receipt_deleted.notified().await;

            // Wait for the receipt-triggered DELETE to be acknowledged locally.
            while db
                .list_automations_for_sync()
                .iter()
                .any(|row| row.id == first.id)
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        std::env::remove_var("CODEMUX_API_URL");
        server.abort();
        let _ = server.await;
        result.unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn an_account_switch_during_pull_discards_the_reply_and_stops_writes() {
        use crate::auth::save_token;

        let app = tauri::test::mock_app();
        app.manage(init_test_database());
        app.manage(AuthState::default());
        let db = app.state::<DatabaseStore>();
        let expiry = "2099-01-01T00:00:00Z";
        save_token(&db, "account-a", expiry).unwrap();

        let mut server = mockito::Server::new_async().await;
        let list = server
            .mock("GET", "/api/automations")
            .match_header("authorization", "Bearer account-a")
            .with_body_from_request({
                let app = app.handle().clone();
                move |_| {
                    let db = app.state::<DatabaseStore>();
                    app.state::<AuthState>().replace_session(|| {
                        save_token(&db, "account-b", expiry).unwrap();
                        db.insert_automation(&sample_input()).unwrap();
                    });
                    format!(
                        r#"{{"automations":[{}]}}"#,
                        server_copy("srv-a", "Account A")
                    )
                    .into_bytes()
                }
            })
            .create_async()
            .await;
        let post = server
            .mock("POST", "/api/automations")
            .expect(0)
            .create_async()
            .await;

        std::env::set_var("CODEMUX_API_URL", server.url());
        let result = try_sync_with_app(app.handle()).await;
        std::env::remove_var("CODEMUX_API_URL");

        assert!(result.unwrap_err().contains("account changed"));
        list.assert_async().await;
        post.assert_async().await;
        let rows = db.list_automations_for_sync();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, sample_input().name);
        assert!(rows[0].dirty);
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
