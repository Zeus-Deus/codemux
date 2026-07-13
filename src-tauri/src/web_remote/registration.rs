//! Desktop device registration with the account control plane.
//!
//! When the from-anywhere iroh transport (`relay_mode_enabled`) turns on and
//! the desktop is signed into a Codemux account, the desktop registers itself
//! with the account device registry so a browser signed into the **same**
//! account can discover it — and dial it by `node_id` — from any network. This
//! is the desktop half of the Design 1 Stage B/C control-plane registry
//! (`docs/plans/web-remote-account-mode.md` §1.2).
//!
//! ## Contract
//!
//! `POST /api/devices {deviceId, nodeId, name, platform}` (camelCase, upsert),
//! authenticated by the desktop's own account bearer token. The registry keys
//! on the stable `deviceId` (persisted locally, generated once), so re-running
//! the POST just refreshes `lastSeenAt` — that is exactly the periodic refresh
//! this module runs while relay mode is up.
//!
//! ## Discipline
//!
//! Strictly best-effort and non-fatal: signed out → skip (nothing to register
//! against); API unreachable / non-2xx → log + record the error, never crash or
//! block the enable path. Registration adds reachability metadata to the
//! control plane; it never gates the local server or the iroh transport.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager};

use super::Shared;
use crate::database::DatabaseStore;

/// Settings key under which the stable per-install device id is persisted. The
/// registry upserts on this id, so it must survive restarts (a fresh id would
/// register a duplicate device row on every boot).
const DEVICE_ID_KEY: &str = "web_remote.device_id";

/// How often the desktop re-POSTs `/api/devices` to refresh `lastSeenAt` while
/// relay mode is up. Long enough to be negligible load, short enough that the
/// registry's "last seen" is a useful liveness signal for the device picker.
const REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// The `POST /api/devices` request body. camelCase on the wire per the shared
/// device-registry contract (the rest of the desktop's Tauri surface is
/// snake_case; only this API body is camelCase).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeviceRegistration {
    pub device_id: String,
    pub node_id: String,
    pub name: String,
    pub platform: String,
}

/// Registration status surfaced to the desktop UI (via `web_remote_status` and
/// the dedicated `web_remote_registration_status` command). snake_case JSON,
/// matching the rest of the desktop command surface.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RegistrationStatus {
    /// Whether the last registration attempt succeeded (the desktop is
    /// discoverable in the account device registry). `false` when signed out,
    /// relay mode is off, or the registry is unreachable.
    pub registered: bool,
    /// The stable device id this desktop registers under. `None` until the
    /// first registration attempt runs (it is generated + persisted then).
    pub device_id: Option<String>,
    /// The iroh `node_id` last registered (the address a browser dials).
    pub node_id: Option<String>,
    /// When the last successful registration happened (RFC3339). `None` if none
    /// has succeeded this run.
    pub last_registered_at: Option<String>,
    /// The last registration error, for diagnostics. `None` on success or when
    /// registration has not been attempted.
    pub last_error: Option<String>,
}

#[derive(Default)]
struct RegistrationInner {
    status: RegistrationStatus,
    /// The periodic-refresh task, kept so `stop` can abort it. `Some` while
    /// relay-mode registration is active.
    refresh: Option<tauri::async_runtime::JoinHandle<()>>,
}

/// Owns the device-registration lifecycle (the periodic refresh task + the last
/// status). Lives inside [`Shared`] so it rides alongside the iroh endpoint's
/// lifecycle — started/stopped in lockstep with relay mode.
#[derive(Default)]
pub struct RegistrationManager {
    inner: Mutex<RegistrationInner>,
}

impl RegistrationManager {
    /// A snapshot of the current registration status.
    pub fn status(&self) -> RegistrationStatus {
        self.inner.lock().unwrap().status.clone()
    }

    fn is_running(&self) -> bool {
        self.inner.lock().unwrap().refresh.is_some()
    }

    fn record_success(&self, reg: &DeviceRegistration) {
        let mut inner = self.inner.lock().unwrap();
        inner.status.registered = true;
        inner.status.device_id = Some(reg.device_id.clone());
        inner.status.node_id = Some(reg.node_id.clone());
        inner.status.last_registered_at = Some(chrono::Utc::now().to_rfc3339());
        inner.status.last_error = None;
    }

    fn record_error(&self, device_id: Option<String>, error: impl Into<String>) {
        let mut inner = self.inner.lock().unwrap();
        inner.status.registered = false;
        if device_id.is_some() {
            inner.status.device_id = device_id;
        }
        inner.status.last_error = Some(error.into());
    }
}

/// The stable device id for this install: read from settings, or generated once
/// and persisted. A persistence failure still returns a usable id (the registry
/// just can't dedupe across restarts in that degenerate case).
fn stable_device_id(db: &DatabaseStore) -> String {
    if let Some(existing) = db.get_setting(DEVICE_ID_KEY) {
        if !existing.trim().is_empty() {
            return existing;
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    if let Err(e) = db.set_setting(DEVICE_ID_KEY, &id) {
        eprintln!("[codemux::web_remote] persisting device id failed: {e}");
    }
    id
}

/// This device's display name for the registry — the machine hostname, falling
/// back to a generic label so a nameless host still registers.
fn device_name() -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Codemux Desktop".to_string())
}

/// Build the registration body from the persisted device id + the live node id.
fn build_registration(db: &DatabaseStore, node_id: &str) -> DeviceRegistration {
    DeviceRegistration {
        device_id: stable_device_id(db),
        node_id: node_id.to_string(),
        name: device_name(),
        platform: std::env::consts::OS.to_string(),
    }
}

/// POST the registration to the control plane with the account bearer. Any
/// non-2xx is an error (the caller logs + records it); a good response body is
/// ignored — the registry keys on the client-supplied `deviceId`.
async fn post_registration(
    base: &str,
    bearer: &str,
    reg: &DeviceRegistration,
) -> Result<(), String> {
    let url = format!("{base}/api/devices");
    let resp = reqwest::Client::new()
        .post(&url)
        .header("Authorization", format!("Bearer {bearer}"))
        .json(reg)
        .send()
        .await
        .map_err(|e| format!("device registration request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("device registration returned {}", resp.status()));
    }
    Ok(())
}

/// A response the API returns from `GET /api/devices` — used only to keep the
/// deserialize shape pinned for tests; the desktop registers, it does not list.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct DeviceListResponse {
    devices: Vec<Value>,
}

/// Run one registration attempt: gather the node id + bearer + device id, POST,
/// and update the status. Signed out (no bearer) or no node id → skip quietly
/// (record the reason, don't error). Never panics.
async fn register_once(app: &AppHandle, shared: &Arc<Shared>) {
    // Snapshot every input synchronously so no DB/State guard is held across
    // the network await below (mirrors the account-mode admission discipline).
    let (bearer, reg) = {
        let db = app.state::<DatabaseStore>();
        let bearer = crate::auth::load_token(&db).map(|(t, _)| t);
        // Build the body (and thus persist/read the stable device id) up front
        // so the status can report the device id even on the skip paths.
        let reg = shared
            .iroh
            .node_id()
            .map(|nid| build_registration(&db, &nid));
        (bearer, reg)
    };

    let reg = match reg {
        Some(r) => r,
        None => {
            // Relay mode's identity key hasn't been generated yet — nothing to
            // register. A later refresh tick will pick it up.
            shared
                .registration
                .record_error(None, "no iroh node_id yet");
            return;
        }
    };

    let bearer = match bearer {
        None => {
            // Signed out: there is no account to register against. Not an error
            // condition — just nothing to do.
            shared
                .registration
                .record_error(Some(reg.device_id.clone()), "desktop signed out");
            return;
        }
        Some(b) => b,
    };

    let base = crate::auth::api_base_url();
    match post_registration(&base, &bearer, &reg).await {
        Ok(()) => shared.registration.record_success(&reg),
        Err(e) => {
            eprintln!("[codemux::web_remote] {e}");
            shared
                .registration
                .record_error(Some(reg.device_id.clone()), e);
        }
    }
    super::emit_state_changed(app);
}

/// Start device registration: register immediately, then refresh `lastSeenAt`
/// on [`REFRESH_INTERVAL`] until [`stop`]. Idempotent — a second call while the
/// refresh task is live is a no-op. Non-blocking (the initial POST runs on the
/// spawned task), so enabling relay mode never waits on the network.
pub(crate) fn start(app: &AppHandle, shared: &Arc<Shared>) {
    if shared.registration.is_running() {
        return;
    }
    let app = app.clone();
    let shared_task = shared.clone();
    let handle = tauri::async_runtime::spawn(async move {
        loop {
            register_once(&app, &shared_task).await;
            tokio::time::sleep(REFRESH_INTERVAL).await;
        }
    });
    shared.registration.inner.lock().unwrap().refresh = Some(handle);
}

/// Stop device registration: abort the refresh task and mark the desktop
/// un-registered. Safe to call when not running. The device row is left in the
/// registry (it goes stale via `lastSeenAt` and the picker filters on that);
/// hard de-registration is an account-side action (`DELETE /api/devices/:id`).
pub(crate) fn stop(shared: &Arc<Shared>) {
    let mut inner = shared.registration.inner.lock().unwrap();
    if let Some(handle) = inner.refresh.take() {
        handle.abort();
    }
    inner.status.registered = false;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{save_auth, AuthUser};
    use crate::database::init_test_database;

    #[test]
    fn registration_body_is_camel_case_with_all_fields() {
        let reg = DeviceRegistration {
            device_id: "dev-1".into(),
            node_id: "node-abc".into(),
            name: "My Laptop".into(),
            platform: "linux".into(),
        };
        let v = serde_json::to_value(&reg).unwrap();
        assert_eq!(v["deviceId"], "dev-1");
        assert_eq!(v["nodeId"], "node-abc");
        assert_eq!(v["name"], "My Laptop");
        assert_eq!(v["platform"], "linux");
        // Exactly the four contract fields, camelCase — no snake_case leakage.
        assert!(v.get("device_id").is_none(), "no snake_case leakage");
        assert!(v.get("node_id").is_none());
        assert_eq!(v.as_object().unwrap().len(), 4);
    }

    #[test]
    fn stable_device_id_persists_and_is_reused() {
        let db = init_test_database();
        let first = stable_device_id(&db);
        assert!(!first.is_empty());
        // A second read returns the SAME id (persisted, not regenerated) — the
        // registry upsert relies on this to avoid duplicate device rows.
        let second = stable_device_id(&db);
        assert_eq!(first, second, "device id must be stable across calls");
        assert_eq!(db.get_setting(DEVICE_ID_KEY).as_deref(), Some(first.as_str()));
    }

    #[test]
    fn build_registration_carries_node_id_and_current_platform() {
        let db = init_test_database();
        let reg = build_registration(&db, "node-xyz");
        assert_eq!(reg.node_id, "node-xyz");
        assert_eq!(reg.platform, std::env::consts::OS);
        assert!(!reg.device_id.is_empty());
        assert!(!reg.name.is_empty());
    }

    // The registration POST is exercised against a mocked control plane
    // (mockito) and NEVER the real API. `CODEMUX_API_URL` mutation is
    // serialized with the other `mockserver` tests.

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn post_registration_sends_camel_case_body_with_bearer() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/devices")
            .match_header("authorization", "Bearer test-bearer")
            .match_body(mockito::Matcher::PartialJsonString(
                serde_json::json!({
                    "deviceId": "dev-42",
                    "nodeId": "node-42",
                    "platform": "linux",
                })
                .to_string(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body("{}")
            .create_async()
            .await;

        let reg = DeviceRegistration {
            device_id: "dev-42".into(),
            node_id: "node-42".into(),
            name: "Host".into(),
            platform: "linux".into(),
        };
        let res = post_registration(&server.url(), "test-bearer", &reg).await;
        assert!(res.is_ok(), "200 registration succeeds: {res:?}");
        mock.assert_async().await;
    }

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn post_registration_errors_on_unauthorized() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/api/devices")
            .with_status(401)
            .with_body(r#"{"error":"unauthorized"}"#)
            .create_async()
            .await;

        let reg = DeviceRegistration {
            device_id: "dev-1".into(),
            node_id: "node-1".into(),
            name: "Host".into(),
            platform: "linux".into(),
        };
        let err = post_registration(&server.url(), "bad-bearer", &reg)
            .await
            .expect_err("a 401 must be an error, not a silent success");
        assert!(err.contains("401"), "error names the status: {err}");
    }

    #[test]
    fn record_success_then_error_tracks_status() {
        let mgr = RegistrationManager::default();
        assert!(!mgr.status().registered);
        assert!(mgr.status().device_id.is_none());

        let reg = DeviceRegistration {
            device_id: "dev-9".into(),
            node_id: "node-9".into(),
            name: "Host".into(),
            platform: "linux".into(),
        };
        mgr.record_success(&reg);
        let ok = mgr.status();
        assert!(ok.registered);
        assert_eq!(ok.device_id.as_deref(), Some("dev-9"));
        assert_eq!(ok.node_id.as_deref(), Some("node-9"));
        assert!(ok.last_registered_at.is_some());
        assert!(ok.last_error.is_none());

        mgr.record_error(Some("dev-9".into()), "device registration returned 500");
        let bad = mgr.status();
        assert!(!bad.registered, "an error clears the registered flag");
        assert_eq!(bad.last_error.as_deref(), Some("device registration returned 500"));
        // The device id survives the error so the UI can still show it.
        assert_eq!(bad.device_id.as_deref(), Some("dev-9"));
    }

    #[test]
    fn device_list_response_shape_deserializes() {
        // Pins the `GET /api/devices` envelope shape from the shared contract so
        // a drift in the wrapper key is caught here rather than at runtime.
        let body = serde_json::json!({
            "devices": [
                { "id": "row-1", "deviceId": "dev-1", "nodeId": "n1", "name": "A", "platform": "linux" }
            ]
        })
        .to_string();
        let parsed: DeviceListResponse = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed.devices.len(), 1);
    }

    // A helper mirroring the account-mode tests: seed the desktop's cached
    // account user so the "signed in" branch is reachable in future wiring.
    #[allow(dead_code)]
    fn seed_cached_user(db: &DatabaseStore, id: &str) {
        let user = AuthUser {
            id: id.to_string(),
            email: "desktop@example.com".to_string(),
            name: Some("Desktop User".to_string()),
            image: None,
        };
        save_auth(db, "desktop-token", "2099-01-01T00:00:00Z", Some(&user)).unwrap();
    }

    #[test]
    fn load_token_bearer_is_present_when_signed_in() {
        // register_once reads the account bearer via `load_token`; confirm a
        // signed-in desktop yields one (the "signed out → skip" branch is the
        // complement).
        let db = init_test_database();
        assert!(crate::auth::load_token(&db).is_none(), "signed out by default");
        seed_cached_user(&db, "user-1");
        let bearer = crate::auth::load_token(&db).map(|(t, _)| t);
        assert_eq!(bearer.as_deref(), Some("desktop-token"));
    }
}
