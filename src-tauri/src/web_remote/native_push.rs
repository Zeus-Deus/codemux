//! Native phone-app push, delivered through the Codemux push relay.
//!
//! APNs and FCM credentials can't ship inside a desktop app, so the relay
//! holds them. The phone registers its native token with the relay itself
//! and receives a handle: a `device_id` plus a `send_key` capability. It
//! hands that handle to each paired desktop over its authenticated remote
//! session (`web_push_native_*`, routed per session like `web_push_*`), and
//! the desktop posts notifications to the relay with it. The desktop never
//! sees the device token, and the relay never sees chat content beyond the
//! short title and body it is asked to show.
//!
//! The relay is optional. Without one configured (`CODEMUX_PUSH_RELAY_URL`
//! or the `mobile_push.relay_url` setting) registration is refused with a
//! clear reason, the phone app stays live only while open, and browser Web
//! Push keeps working exactly as before. The contract lives in
//! `services/push-relay/README.md`.
use crate::database::{mobile_push::MobilePushDevice, DatabaseStore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{collections::BTreeMap, sync::OnceLock, time::Duration};
use tauri::{AppHandle, Manager, Runtime};

pub const RELAY_URL_SETTING: &str = "mobile_push.relay_url";
const RELAY_URL_ENV: &str = "CODEMUX_PUSH_RELAY_URL";
const SEND_TIMEOUT: Duration = Duration::from_secs(15);
static SEND_LIMIT: OnceLock<tokio::sync::Semaphore> = OnceLock::new();

fn yes() -> bool {
    true
}

/// What a phone wants to hear about. `attention` covers approvals and
/// questions; `pr` covers PR-watch news (failed checks, reviews, conflicts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Categories {
    #[serde(default = "yes")]
    attention: bool,
    #[serde(default = "yes")]
    complete: bool,
    #[serde(default = "yes")]
    failure: bool,
    #[serde(default = "yes")]
    pr: bool,
}

impl Default for Categories {
    fn default() -> Self {
        Self { attention: true, complete: true, failure: true, pr: true }
    }
}

impl Categories {
    fn allows(&self, kind: Kind) -> bool {
        match kind {
            Kind::Approval => self.attention,
            Kind::Complete => self.complete,
            Kind::Failure => self.failure,
            Kind::Pr => self.pr,
            Kind::Test => true,
        }
    }
}

/// The relay's notification categories (its contract, not ours).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Approval,
    Complete,
    Failure,
    Pr,
    Test,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Approval => "approval",
            Kind::Complete => "complete",
            Kind::Failure => "failure",
            Kind::Pr => "pr",
            Kind::Test => "test",
        }
    }

    /// The `push.rs` category names shared with browser Web Push.
    pub fn from_web_category(category: &str) -> Option<Self> {
        match category {
            "attention" => Some(Kind::Approval),
            "complete" => Some(Kind::Complete),
            "failure" => Some(Kind::Failure),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Notice {
    pub kind: Kind,
    pub title: String,
    pub body: String,
    /// Replaces an earlier notice with the same key on the phone.
    pub collapse_key: String,
    /// Routing ids for the tap (workspace, pane, thread). Strings only.
    pub data: BTreeMap<String, String>,
}

/// Only an https relay, or plain http on loopback for local development.
/// The phone supplies the handle but never the URL: the desktop posts only to
/// the relay its owner configured, so a paired client can't aim it elsewhere.
pub fn normalize_relay_url(raw: &str) -> Option<String> {
    let url = reqwest::Url::parse(raw.trim()).ok()?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    Some(url.as_str().trim_end_matches('/').to_string())
}

pub fn relay_url(db: &DatabaseStore) -> Option<String> {
    std::env::var(RELAY_URL_ENV)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| db.get_setting(RELAY_URL_SETTING))
        .and_then(|raw| normalize_relay_url(&raw))
}

/// Relay ids and keys are base64url; anything else is refused before it can
/// become part of a URL path or a header.
fn valid_token(value: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&value.len())
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn categories_of(device: &MobilePushDevice) -> Categories {
    serde_json::from_str(&device.categories).unwrap_or_default()
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn parse_categories(value: &Value) -> Result<Categories, String> {
    if value.is_null() {
        return Ok(Categories::default());
    }
    serde_json::from_value(value.clone()).map_err(|_| "Invalid notification preferences".into())
}

/// `web_push_native_*`, already bound to the caller's approved session.
pub async fn invoke<R: Runtime>(
    app: &AppHandle<R>,
    session: &str,
    cmd: &str,
    args: &Value,
) -> Result<Value, String> {
    let db = app.state::<DatabaseStore>();
    let relay = relay_url(&db);
    let current = db.mobile_push_device(session)?;
    match cmd {
        "web_push_native_status" => {
            let registered = current
                .as_ref()
                .filter(|d| Some(&d.relay_url) == relay.as_ref());
            Ok(json!({
                "relay_url": relay,
                "registered": registered.is_some(),
                "device_id": registered.map(|d| d.device_id.clone()),
                "categories": registered.map(categories_of),
            }))
        }
        "web_push_native_register" => {
            let relay = relay.ok_or("This desktop has no push relay configured")?;
            let requested = args["relayUrl"].as_str().and_then(normalize_relay_url);
            if requested.as_deref() != Some(relay.as_str()) {
                return Err("This desktop uses a different push relay".into());
            }
            let device_id = args["deviceId"].as_str().filter(|v| valid_token(v, 16, 64));
            let send_key = args["sendKey"].as_str().filter(|v| valid_token(v, 32, 128));
            let (Some(device_id), Some(send_key)) = (device_id, send_key) else {
                return Err("Invalid push relay handle".into());
            };
            let platform = match args["platform"].as_str() {
                Some(p @ ("ios" | "android")) => p,
                _ => return Err("Invalid platform".into()),
            };
            let label = args["label"]
                .as_str()
                .map(|s| s.trim().chars().take(80).collect::<String>())
                .filter(|s| !s.is_empty());
            let categories = parse_categories(&args["categories"])?;
            let now = now_ms();
            db.upsert_mobile_push_device(&MobilePushDevice {
                session_id: session.into(),
                relay_url: relay,
                device_id: device_id.into(),
                send_key: send_key.into(),
                platform: platform.into(),
                label,
                categories: serde_json::to_string(&categories).map_err(|e| e.to_string())?,
                created_at_ms: now,
                updated_at_ms: now,
            })?;
            // The desktop's device list shows which phones get push.
            super::emit_state_changed(app);
            Ok(json!(null))
        }
        "web_push_native_preferences" => {
            let categories = parse_categories(&args["categories"])?;
            let encoded = serde_json::to_string(&categories).map_err(|e| e.to_string())?;
            if !db.set_mobile_push_categories(session, &encoded, now_ms())? {
                return Err("Enable notifications first".into());
            }
            Ok(json!(null))
        }
        "web_push_native_unregister" => {
            if db.delete_mobile_push_device(session)? {
                super::emit_state_changed(app);
            }
            Ok(json!(null))
        }
        "web_push_native_test" => {
            let device = current.ok_or("Enable notifications first")?;
            let relay = relay.ok_or("This desktop has no push relay configured")?;
            let notice = Notice {
                kind: Kind::Test,
                title: "Codemux is connected".into(),
                body: format!("Notifications from {} are ready.", super::registration::device_name()),
                collapse_key: "codemux-test".into(),
                data: BTreeMap::new(),
            };
            send(app, &relay, &device, &notice).await.map(|_| json!(null))
        }
        _ => Err("Unknown push command".into()),
    }
}

/// The relay request for one device. Pure so the wire shape is testable.
fn request(relay: &str, device: &MobilePushDevice, notice: &Notice) -> (String, Value) {
    let mut data: Map<String, Value> = notice
        .data
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect();
    // The phone keeps one connection per desktop keyed by this session, so a
    // tap knows which machine the workspace lives on.
    data.insert("sessionId".into(), Value::String(device.session_id.clone()));
    let url = format!("{relay}/v1/devices/{}/notifications", device.device_id);
    // The relay counts these limits in code points too (`chars`, not UTF-16).
    let body = json!({
        "title": notice.title.chars().take(120).collect::<String>(),
        "body": notice.body.chars().take(400).collect::<String>(),
        "category": notice.kind.as_str(),
        // The relay sends it as an HTTP header, so it only takes a safe set.
        "collapseKey": notice
            .collapse_key
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-') { c } else { '-' })
            .take(64)
            .collect::<String>(),
        "data": data,
        "ttlSeconds": 3600,
    });
    (url, body)
}

/// The registration a queued send was planned against still stands and
/// still wants this kind of notice.
fn still_wanted(current: &MobilePushDevice, planned: &MobilePushDevice, relay: &str, kind: Kind) -> bool {
    current.relay_url == relay
        && current.device_id == planned.device_id
        && current.send_key == planned.send_key
        && categories_of(current).allows(kind)
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Delivered,
    /// The relay no longer knows this handle or key: forget it.
    Gone,
    Failed,
}

fn classify(status: u16) -> Outcome {
    match status {
        200..=299 => Outcome::Delivered,
        401 | 404 | 410 => Outcome::Gone,
        _ => Outcome::Failed,
    }
}

fn remote_access_on<R: Runtime>(app: &AppHandle<R>) -> bool {
    let shared = app.state::<super::WebRemoteState>().shared();
    let config = shared.config.lock().unwrap();
    config.enabled || config.relay_mode_enabled
}

async fn send<R: Runtime>(
    app: &AppHandle<R>,
    relay: &str,
    device: &MobilePushDevice,
    notice: &Notice,
) -> Result<(), String> {
    let _permit = SEND_LIMIT
        .get_or_init(|| tokio::sync::Semaphore::new(4))
        .acquire()
        .await
        .map_err(|_| "Push delivery stopped")?;
    if !super::push::active_session(app, &device.session_id) {
        return Err("Remote session is no longer approved".into());
    }
    if !remote_access_on(app) {
        return Err("Remote access is disabled".into());
    }
    // Sends can queue behind slow ones: an unregister, re-pair or category
    // change made meanwhile wins over the snapshot this send started with.
    let current = app.state::<DatabaseStore>().mobile_push_device(&device.session_id)?;
    if !current.as_ref().is_some_and(|c| still_wanted(c, device, relay, notice.kind)) {
        return Ok(());
    }
    let (url, body) = request(relay, device, notice);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(SEND_TIMEOUT)
        .build()
        .map_err(|_| "Cannot start push client")?;
    let response = client
        .post(url)
        .bearer_auth(&device.send_key)
        .json(&body)
        .send()
        .await
        .map_err(|_| "Push relay is unreachable")?;
    let status = response.status().as_u16();
    match classify(status) {
        Outcome::Delivered => Ok(()),
        Outcome::Gone => {
            app.state::<DatabaseStore>().delete_mobile_push_handle(
                &device.relay_url,
                &device.device_id,
                &device.send_key,
            )?;
            Err("This phone is no longer registered for notifications. Open the app to enable them again.".into())
        }
        Outcome::Failed => Err(format!("Push relay returned {status}")),
    }
}

/// Fan a notice out to every registered phone that wants it. Fire and forget:
/// a slow or missing relay never holds up the agent event that caused it.
pub fn deliver<R: Runtime>(app: &AppHandle<R>, notice: Notice) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let db = app.state::<DatabaseStore>();
        let Some(relay) = relay_url(&db) else {
            return;
        };
        let devices = match db.mobile_push_devices() {
            Ok(devices) => devices,
            Err(error) => {
                eprintln!("[native-push] {error}");
                return;
            }
        };
        for device in devices {
            if !super::push::active_session(&app, &device.session_id) {
                let _ = db.delete_mobile_push_device(&device.session_id);
                continue;
            }
            if device.relay_url != relay || !categories_of(&device).allows(notice.kind) {
                continue;
            }
            if let Err(error) = send(&app, &relay, &device, &notice).await {
                eprintln!("[native-push] {error}");
            }
        }
    });
}

/// PR-watch news for the chat's phone owners. The sweep has already
/// deduplicated it against what the chat was told, so this only resolves
/// where the chat lives and respects a muted workspace.
pub fn pr_event<R: Runtime>(
    app: &AppHandle<R>,
    watch: &crate::database::pr_watch::PrWatchRecord,
    summary: &str,
) {
    let state = app.state::<crate::state::AppStateStore>();
    let target = state.notification_target_for_thread(&watch.thread_id).or_else(|| {
        let workspace = state.find_workspace(&watch.workspace_id)?;
        Some(crate::state::NotificationTarget {
            workspace_id: workspace.workspace_id.0.clone(),
            workspace_title: workspace.title.clone(),
            pane_id: String::new(),
            muted: workspace.notifications_muted,
            in_active_workspace: false,
        })
    });
    let Some(target) = target.filter(|t| !t.muted) else {
        return;
    };
    deliver(app, pr_notice(&watch.thread_id, watch.pr_number, &target, summary));
}

fn pr_notice(
    thread_id: &str,
    pr_number: u32,
    target: &crate::state::NotificationTarget,
    summary: &str,
) -> Notice {
    let mut data = BTreeMap::from([
        ("workspaceId".to_string(), target.workspace_id.clone()),
        ("threadId".to_string(), thread_id.to_string()),
        ("prNumber".to_string(), pr_number.to_string()),
    ]);
    if !target.pane_id.is_empty() {
        data.insert("paneId".into(), target.pane_id.clone());
    }
    Notice {
        kind: Kind::Pr,
        title: format!("PR #{pr_number} · {}", target.workspace_title),
        body: summary.to_string(),
        collapse_key: format!("pr-{}-{pr_number}", target.workspace_id),
        data,
    }
}

/// Paired sessions running the phone app with push on, for the desktop's
/// device list. The send key never leaves the backend.
#[tauri::command]
pub fn web_remote_push_devices(
    db: tauri::State<'_, DatabaseStore>,
) -> Result<Vec<MobilePushDevice>, String> {
    db.mobile_push_devices()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> MobilePushDevice {
        MobilePushDevice {
            session_id: "sess".into(),
            relay_url: "https://relay.test/api/push".into(),
            device_id: "AAAAAAAAAAAAAAAAAAAAAA".into(),
            send_key: "k".repeat(43),
            platform: "ios".into(),
            label: None,
            categories: r#"{"attention":true,"complete":false,"failure":true}"#.into(),
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    /// A local stand-in for the relay: records each request and answers
    /// with whatever status the test sets.
    async fn fake_relay() -> (
        String,
        std::sync::Arc<std::sync::Mutex<Vec<(String, String, Value)>>>,
        std::sync::Arc<std::sync::atomic::AtomicU16>,
    ) {
        use axum::{extract::State, http::HeaderMap, routing::post, Json, Router};
        use std::sync::{atomic::AtomicU16, Arc, Mutex};
        type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;
        let seen: Seen = Arc::default();
        let status = Arc::new(AtomicU16::new(202));
        let app = Router::new()
            .route(
                "/push/v1/devices/:id/notifications",
                post(
                    |State((seen, status)): State<(Seen, Arc<AtomicU16>)>,
                     axum::extract::Path(id): axum::extract::Path<String>,
                     headers: HeaderMap,
                     Json(body): Json<Value>| async move {
                        let auth = headers["authorization"].to_str().unwrap().to_string();
                        seen.lock().unwrap().push((id, auth, body));
                        axum::http::StatusCode::from_u16(status.load(std::sync::atomic::Ordering::SeqCst)).unwrap()
                    },
                ),
            )
            .with_state((seen.clone(), status.clone()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://127.0.0.1:{}/push", addr.port()), seen, status)
    }

    #[tokio::test]
    async fn register_test_and_forget_a_gone_phone_through_the_relay() {
        let app = tauri::test::mock_app();
        let db = DatabaseStore::new_in_memory();
        db.web_remote_insert_session("phone", Some("Pixel"), None, "hash", true).unwrap();
        let (relay, seen, status) = fake_relay().await;
        db.set_setting(RELAY_URL_SETTING, &relay).unwrap();
        app.manage(db);
        app.manage(super::super::WebRemoteState::default());
        // Hydrate from the (empty) settings first, as the app does on boot,
        // so a later status read can't reset the config under the test.
        let shared = app.state::<super::super::WebRemoteState>().shared();
        super::super::ensure_config_hydrated(app.handle(), &shared);
        shared.config.lock().unwrap().enabled = true;
        let handle = app.handle();
        let key = "k".repeat(43);

        let before = invoke(handle, "phone", "web_push_native_status", &json!({})).await.unwrap();
        assert_eq!(before["relay_url"], json!(relay));
        assert_eq!(before["registered"], json!(false));

        // The phone can't aim the desktop at a relay of its choosing.
        let elsewhere = json!({"relayUrl": "https://evil.test", "deviceId": "AAAAAAAAAAAAAAAAAAAAAA", "sendKey": key, "platform": "ios"});
        assert!(invoke(handle, "phone", "web_push_native_register", &elsewhere).await.is_err());

        let register = json!({
            "relayUrl": format!("{relay}/"),
            "deviceId": "AAAAAAAAAAAAAAAAAAAAAA",
            "sendKey": key,
            "platform": "ios",
            "label": "Pixel",
            "categories": {"attention": true, "complete": false, "failure": true, "pr": true},
        });
        invoke(handle, "phone", "web_push_native_register", &register).await.unwrap();
        let after = invoke(handle, "phone", "web_push_native_status", &json!({})).await.unwrap();
        assert_eq!(after["registered"], json!(true));
        assert_eq!(after["categories"]["complete"], json!(false));

        invoke(handle, "phone", "web_push_native_test", &json!({})).await.unwrap();
        {
            let seen = seen.lock().unwrap();
            let (id, auth, body) = &seen[0];
            assert_eq!(id, "AAAAAAAAAAAAAAAAAAAAAA");
            assert_eq!(auth, &format!("Bearer {key}"));
            assert_eq!(body["category"], "test");
            assert_eq!(body["data"]["sessionId"], "phone");
        }

        // The relay forgot the phone: the desktop stops trying.
        status.store(410, std::sync::atomic::Ordering::SeqCst);
        assert!(invoke(handle, "phone", "web_push_native_test", &json!({})).await.is_err());
        assert!(app.state::<DatabaseStore>().mobile_push_device("phone").unwrap().is_none());

        // A revoked session can no longer manage push at all.
        invoke(handle, "phone", "web_push_native_register", &register).await.unwrap();
        app.state::<DatabaseStore>().web_remote_revoke_session("phone").unwrap();
        assert!(super::super::push::invoke(handle, "phone", "web_push_native_status", &json!({})).await.is_err());
    }

    #[test]
    fn relay_url_must_be_https_or_loopback_http() {
        assert_eq!(
            normalize_relay_url(" https://api.codemux.org/api/push/ "),
            Some("https://api.codemux.org/api/push".into())
        );
        assert_eq!(normalize_relay_url("http://localhost:8787"), Some("http://localhost:8787".into()));
        for bad in [
            "http://api.codemux.org/api/push",
            "https://user:pw@api.codemux.org",
            "https://api.codemux.org/push?x=1",
            "https://api.codemux.org/push#frag",
            "ftp://api.codemux.org",
            "not a url",
        ] {
            assert_eq!(normalize_relay_url(bad), None, "{bad}");
        }
    }

    #[test]
    fn handles_are_base64url_and_bounded() {
        assert!(valid_token("abcDEF012-_abcdef", 16, 64));
        assert!(!valid_token("short", 16, 64));
        assert!(!valid_token("abcdefghijklmnop/../x", 16, 64));
        assert!(!valid_token(&"a".repeat(65), 16, 64));
    }

    #[test]
    fn stored_preferences_gate_categories_and_default_missing_ones_on() {
        let categories = categories_of(&device());
        assert!(categories.allows(Kind::Approval));
        assert!(!categories.allows(Kind::Complete));
        assert!(categories.allows(Kind::Failure));
        // Stored before `pr` existed: on by default.
        assert!(categories.allows(Kind::Pr));
        assert!(categories.allows(Kind::Test));
        let broken = MobilePushDevice { categories: "nope".into(), ..device() };
        assert_eq!(categories_of(&broken), Categories::default());
    }

    #[test]
    fn request_carries_the_session_and_clips_text() {
        let notice = Notice {
            kind: Kind::Approval,
            title: "t".repeat(200),
            body: "b".repeat(500),
            collapse_key: format!("ws 1/{}", "c".repeat(80)),
            data: BTreeMap::from([("workspaceId".to_string(), "ws".to_string())]),
        };
        let (url, body) = request("https://relay.test/api/push", &device(), &notice);
        assert_eq!(url, "https://relay.test/api/push/v1/devices/AAAAAAAAAAAAAAAAAAAAAA/notifications");
        assert_eq!(body["category"], "approval");
        assert_eq!(body["title"].as_str().unwrap().len(), 120);
        assert_eq!(body["body"].as_str().unwrap().len(), 400);
        assert_eq!(body["collapseKey"].as_str().unwrap().len(), 64);
        assert!(body["collapseKey"].as_str().unwrap().starts_with("ws-1-ccc"));
        assert_eq!(body["data"], json!({"workspaceId": "ws", "sessionId": "sess"}));
        // The capability travels only in the Authorization header.
        assert!(!body.to_string().contains(&device().send_key));
    }

    #[test]
    fn request_clips_text_by_code_point() {
        let notice = Notice {
            kind: Kind::Complete,
            title: "😀".repeat(200),
            body: "😀".repeat(500),
            collapse_key: "c".into(),
            data: BTreeMap::new(),
        };
        let (_, body) = request("https://relay.test/api/push", &device(), &notice);
        assert_eq!(body["title"].as_str().unwrap().chars().count(), 120);
        assert_eq!(body["body"].as_str().unwrap().chars().count(), 400);
    }

    #[test]
    fn a_queued_send_yields_to_later_changes() {
        let relay = "https://relay.test/api/push";
        let planned = device();
        assert!(still_wanted(&planned, &planned, relay, Kind::Approval));
        // Category switched off while queued.
        assert!(!still_wanted(&planned, &planned, relay, Kind::Complete));
        // Re-registered with a new handle, or under another relay.
        let rekeyed = MobilePushDevice { send_key: "n".repeat(43), ..device() };
        assert!(!still_wanted(&rekeyed, &planned, relay, Kind::Approval));
        assert!(!still_wanted(&planned, &planned, "https://other.test", Kind::Approval));
    }

    #[test]
    fn relay_statuses_map_to_keep_forget_or_retry() {
        assert_eq!(classify(202), Outcome::Delivered);
        for gone in [401, 404, 410] {
            assert_eq!(classify(gone), Outcome::Gone);
        }
        for failed in [400, 429, 500, 502, 503] {
            assert_eq!(classify(failed), Outcome::Failed);
        }
    }

    #[test]
    fn pr_notice_names_the_pull_request_and_workspace() {
        let target = crate::state::NotificationTarget {
            workspace_id: "ws".into(),
            workspace_title: "Checkout".into(),
            pane_id: String::new(),
            muted: false,
            in_active_workspace: false,
        };
        let notice = pr_notice("thread", 42, &target, "2 checks failed");
        assert_eq!(notice.kind, Kind::Pr);
        assert_eq!(notice.title, "PR #42 · Checkout");
        assert_eq!(notice.body, "2 checks failed");
        assert_eq!(notice.collapse_key, "pr-ws-42");
        assert!(!notice.data.contains_key("paneId"));
        assert_eq!(notice.data["threadId"], "thread");
    }
}
