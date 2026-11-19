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

use crate::sync_guard::{sync_client, SyncGuard, SYNC_BUDGET};
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
/// One sync pass at a time. Without it a second pass — a background tick
/// and "Sync now", or two edits each scheduling a sync — could settle a
/// deletion whose row the first pass is still uploading, before that
/// upload returns the server id the DELETE needs.
static SYNC_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub async fn try_sync_with_app<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<(), String> {
    // Queue before reading the session: a pass that waited out a sign-out
    // or account switch must sync whoever is signed in now, not upload the
    // new account's automations with the old account's token.
    let _one_at_a_time = SYNC_LOCK.lock().await;
    let db = app.state::<DatabaseStore>();
    let auth = app.state::<crate::auth::AuthState>();
    let (generation, token) = auth.session_snapshot(|| valid_token(&db));
    let Some(token) = token else {
        return Ok(());
    };
    let guard = SyncGuard::Session { auth: &auth, generation };
    sync_locked(&token, &db, &guard).await
}

/// One pull-then-push pass, queued behind any other.
pub async fn sync_once(token: &str, db: &DatabaseStore) -> Result<(), String> {
    let _one_at_a_time = SYNC_LOCK.lock().await;
    sync_locked(token, db, &SyncGuard::Unchecked).await
}

/// Pull, then push, within [`SYNC_BUDGET`]. Called with [`SYNC_LOCK`] held.
/// A pass cut off by the budget may have sent an upload whose answer never
/// came: every automation still waiting for its first upload is then
/// treated as possibly created (see [`UNCERTAIN_UPLOADS`]).
async fn sync_locked(token: &str, db: &DatabaseStore, guard: &SyncGuard<'_>) -> Result<(), String> {
    let outcome = tokio::time::timeout(SYNC_BUDGET, async {
        let pull_err = pull_guarded(token, db, None, guard).await.err();
        let push_err = push_guarded(token, db, guard).await.err();
        (pull_err, push_err)
    })
    .await;
    let Ok((pull_err, push_err)) = outcome else {
        let mut uncertain = lock_uncertain();
        for row in db.list_dirty_automations() {
            if row.server_id.is_none() && row.deleted_at.is_none() {
                uncertain.insert(row.id);
            }
        }
        return Err(format!("timed out after {}s", SYNC_BUDGET.as_secs()));
    };
    match (pull_err, push_err) {
        (None, None) => Ok(()),
        (Some(p), None) => Err(format!("pull failed: {p}")),
        (None, Some(p)) => Err(format!("push failed: {p}")),
        (Some(a), Some(b)) => Err(format!("pull failed: {a}; push failed: {b}")),
    }
}

/// Automations whose first upload may have created a server row although
/// no answer said so. The next pull links a matching server row it does not
/// know yet to such an automation instead of adding a duplicate; until a
/// pull has looked, it is neither uploaded again nor settled. In memory
/// only: after a restart it is uploaded again.
static UNCERTAIN_UPLOADS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<i64>>> =
    std::sync::LazyLock::new(Default::default);

fn lock_uncertain() -> std::sync::MutexGuard<'static, std::collections::HashSet<i64>> {
    UNCERTAIN_UPLOADS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Whether the server row `a` is the one an uncertain upload of `row` made.
fn same_automation(row: &AutomationRecord, a: &ServerAutomation) -> bool {
    row.name == a.name
        && row.prompt == a.prompt
        && row.agent == a.agent
        && row.schedule == a.schedule
        && row.timezone == a.timezone
        && row.project_path == a.project_path
        && row.project_remote == a.project_remote
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
    pull_guarded(token, db, host_filter, &SyncGuard::Unchecked).await
}

async fn pull_guarded(
    token: &str,
    db: &DatabaseStore,
    host_filter: Option<&str>,
    guard: &SyncGuard<'_>,
) -> Result<(), String> {
    let base = api_base_url();
    let client = sync_client();
    guard.ensure_current()?;
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
    // Written only while the session that asked is still current.
    guard.commit(|| apply_pulled(db, &body, host_filter.is_none()))?
}

/// `resolve_uploads`: whether this list is the whole account's, so it can
/// settle uploads whose outcome was unknown (a host-filtered one cannot).
fn apply_pulled(db: &DatabaseStore, body: &ListAutomationsResponse, resolve_uploads: bool) -> Result<(), String> {
    let (_local_to_server, server_to_local) = host_maps(db);
    let local = db.list_automations_for_sync();
    let server_ids: std::collections::HashSet<String> =
        body.automations.iter().map(|a| a.id.clone()).collect();

    // An upload whose outcome was unknown: a server row this device does
    // not know yet, matching such an automation, is that upload's result.
    let mut linked: std::collections::HashSet<String> = std::collections::HashSet::new();
    if resolve_uploads {
        let known: std::collections::HashSet<&str> =
            local.iter().filter_map(|r| r.server_id.as_deref()).collect();
        let mut uncertain = lock_uncertain();
        for a in body.automations.iter().filter(|a| !known.contains(a.id.as_str())) {
            if let Some(row) = local
                .iter()
                .find(|r| r.server_id.is_none() && uncertain.contains(&r.id) && same_automation(r, a))
            {
                db.mark_automation_pushed(row, Some(&a.id))?;
                uncertain.remove(&row.id);
                linked.insert(a.id.clone());
            }
        }
        // The list has been seen: an upload that matched nothing made nothing.
        uncertain.clear();
    }

    for a in body.automations.iter().filter(|a| !linked.contains(&a.id)) {
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
    push_guarded(token, db, &SyncGuard::Unchecked).await
}

async fn push_guarded(token: &str, db: &DatabaseStore, guard: &SyncGuard<'_>) -> Result<(), String> {
    let base = api_base_url();
    let client = sync_client();
    let (local_to_server, dirty) = guard.commit(|| (host_maps(db).0, db.list_dirty_automations()))?;
    let mut any_failed = false;
    // The account's list, fetched at most once a pass, and only if a row
    // turns out to be gone.
    let mut listed: Option<Option<std::collections::HashSet<String>>> = None;

    for row in &dirty {
        // Webhook automations stay on this device: the API has no trigger
        // field, so another device would read one as a broken schedule.
        // Clear `dirty` instead, which also lets a tombstone be purged.
        if row.trigger_type == AutomationTrigger::Webhook {
            if let Err(error) = guard.commit(|| db.mark_automation_synced(row.id, None)).and_then(|r| r) {
                eprintln!("[automations-sync] could not settle local id {}: {error}", row.id);
                any_failed = true;
            }
            continue;
        }
        // Waits for a pull to say whether its earlier upload took.
        if row.server_id.is_none() && lock_uncertain().contains(&row.id) {
            continue;
        }
        let result = if row.deleted_at.is_some() {
            push_delete(&client, &base, token, row, db, guard).await
        } else if row.server_id.is_none() {
            push_insert(&client, &base, token, row, &local_to_server, db, guard).await
        } else {
            push_update(&client, &base, token, row, &local_to_server, db, &mut listed, guard).await
        };
        if let Err(error) = result {
            eprintln!("[automations-sync] push failed for local id {}: {error}", row.id);
            any_failed = true;
        }
    }

    guard.commit(|| db.purge_acknowledged_automation_deletes())??;

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
    guard: &SyncGuard<'_>,
) -> Result<(), String> {
    guard.ensure_current()?;
    let resp = match client
        .post(format!("{base}/api/automations"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&upsert_body(row, local_to_server))
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            // Unless it never connected, the server may have made the row.
            if !e.is_connect() {
                lock_uncertain().insert(row.id);
            }
            return Err(format!("Network error: {e}"));
        }
    };
    if !resp.status().is_success() {
        return Err(format!("API error: {}", resp.status()));
    }
    let parsed: OneAutomationResponse = match resp.json().await {
        Ok(parsed) => parsed,
        Err(e) => {
            lock_uncertain().insert(row.id);
            return Err(format!("Parse: {e}"));
        }
    };
    guard.commit(|| db.mark_automation_pushed(row, Some(&parsed.automation.id)))??;
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
    guard: &SyncGuard<'_>,
) -> Result<(), String> {
    let server_id = row
        .server_id
        .as_ref()
        .ok_or_else(|| "push_update called without server_id".to_string())?;
    guard.ensure_current()?;
    let resp = client
        .patch(format!("{base}/api/automations/{server_id}"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&upsert_body(row, local_to_server))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    if resp.status().as_u16() == 404 && server_lacks_automation(client, base, token, server_id, listed, guard).await {
        // Deleted on another device before this edit reached the account.
        // The pull leaves dirty rows alone, so settle it here: the
        // removal wins, as it does for a clean row.
        eprintln!("[automations-sync] server no longer has {server_id}; dropping the local edit");
        let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        return guard.commit(|| {
        db.mark_automation_synced(row.id, None)?;
        db.upsert_automation_from_server(
            server_id,
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
        )
        })?;
    }
    if !resp.status().is_success() {
        return Err(format!("API error: {}", resp.status()));
    }
    guard.commit(|| db.mark_automation_pushed(row, None))??;
    Ok(())
}

/// Whether the account's automation list is readable and has no row
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
    guard: &SyncGuard<'_>,
) -> bool {
    if listed.is_none() {
        let ids = async {
            guard.ensure_current().ok()?;
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
            Some(body.automations.into_iter().map(|a| a.id).collect())
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
    guard: &SyncGuard<'_>,
) -> Result<(), String> {
    let server_id = match &row.server_id {
        Some(sid) => sid,
        None => {
            // Created and deleted before it ever synced — nothing on
            // the server. Clear `dirty` so the tombstone is purged.
            guard.commit(|| db.mark_automation_synced(row.id, None))??;
            return Ok(());
        }
    };
    guard.ensure_current()?;
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
    guard.commit(|| db.mark_automation_pushed(row, None))??;
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

        let after = db
            .list_automations_for_sync()
            .into_iter()
            .find(|r| r.id == row.id)
            .unwrap();
        assert!(after.deleted_at.is_some() && after.dirty, "the deletion still goes up");
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
        db.set_automation_enabled(ids[0], false).unwrap();
        db.delete_automation(ids[1]).unwrap();

        pull("token", &db, None).await.unwrap();

        let rows = db.list_automations_for_sync();
        let row = |id: i64| rows.iter().find(|r| r.id == id).unwrap();
        assert!(!row(ids[0]).enabled && row(ids[0]).dirty, "a pause is not reverted");
        assert!(
            row(ids[1]).deleted_at.is_some() && row(ids[1]).dirty,
            "a deleted automation is not resurrected"
        );
        assert_eq!(row(ids[2]).name, "Untouched", "a clean row still takes the server copy");
        assert_eq!(rows.len(), 3, "no duplicate rows");
        std::env::remove_var("CODEMUX_API_URL");
    }
}
