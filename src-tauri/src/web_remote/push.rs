//! Encrypted Web Push, owned by an approved remote session. The desktop sends
//! directly to the browser's push service; no open phone socket is required.
use crate::{
    database::DatabaseStore,
    state::{AppStateStore, PaneNodeSnapshot, PaneStatus},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::Write,
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager, Runtime};
use web_push::{ContentEncoding, SubscriptionInfo, VapidSignatureBuilder, WebPushMessageBuilder};

static STORE_LOCK: Mutex<()> = Mutex::new(());
static LAST_EVENT: OnceLock<Mutex<HashMap<String, (String, Instant)>>> = OnceLock::new();
static SEND_LIMIT: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
#[derive(Clone, Serialize, Deserialize)]
pub struct Categories {
    attention: bool,
    complete: bool,
    failure: bool,
}
#[derive(Clone, Serialize, Deserialize)]
struct Subscriber {
    session: String,
    subscription: SubscriptionInfo,
    categories: Categories,
    host: String,
}
#[derive(Serialize, Deserialize)]
struct Store {
    private_key: String,
    subscribers: Vec<Subscriber>,
}
fn store_path() -> Result<PathBuf, String> {
    Ok(dirs::config_dir()
        .ok_or("Configuration directory unavailable")?
        .join(crate::APP_DIR_NAME)
        .join("web-push.json"))
}
fn read_store() -> Result<Store, String> {
    let path = store_path()?;
    if path.exists() {
        return serde_json::from_slice(
            &std::fs::read(path).map_err(|_| "Cannot read push configuration")?,
        )
        .map_err(|_| "Invalid push configuration".into());
    }
    let group = openssl::ec::EcGroup::from_curve_name(openssl::nid::Nid::X9_62_PRIME256V1)
        .map_err(|_| "Cannot create push key")?;
    let key = openssl::ec::EcKey::generate(&group).map_err(|_| "Cannot create push key")?;
    let store = Store {
        private_key: URL_SAFE_NO_PAD.encode(
            key.private_key()
                .to_vec_padded(32)
                .map_err(|_| "Cannot encode push key")?,
        ),
        subscribers: vec![],
    };
    write_store(&store)?;
    Ok(store)
}
fn write_store(store: &Store) -> Result<(), String> {
    let path = store_path()?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|_| "Cannot create push directory")?;
    let temp = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temp)
        .map_err(|_| "Cannot save push configuration")?;
    file.write_all(&serde_json::to_vec(store).map_err(|_| "Cannot encode push configuration")?)
        .map_err(|_| "Cannot save push configuration")?;
    file.sync_all()
        .map_err(|_| "Cannot save push configuration")?;
    std::fs::rename(temp, path).map_err(|_| "Cannot save push configuration".into())
}
fn active_session<R: Runtime>(app: &AppHandle<R>, session: &str) -> bool {
    app.state::<DatabaseStore>()
        .web_remote_get_session(session)
        .is_some_and(|s| s.approved && !s.revoked)
}
/// A subscription must never turn the desktop into an arbitrary HTTP client.
fn valid_endpoint(endpoint: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(endpoint) else {
        return false;
    };
    let host = url.host_str().unwrap_or("");
    endpoint.len() <= 4096
        && url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && url.fragment().is_none()
        && (host == "fcm.googleapis.com"
            || host == "updates.push.services.mozilla.com"
            || host.ends_with(".push.apple.com")
            || host.ends_with(".notify.windows.com"))
}
fn valid_subscription(sub: &SubscriptionInfo) -> bool {
    valid_endpoint(&sub.endpoint)
        && URL_SAFE_NO_PAD
            .decode(&sub.keys.auth)
            .is_ok_and(|v| v.len() == 16)
        && URL_SAFE_NO_PAD
            .decode(&sub.keys.p256dh)
            .is_ok_and(|v| v.len() == 65 && v[0] == 4)
}

pub async fn invoke<R: Runtime>(
    app: &AppHandle<R>,
    session: &str,
    cmd: &str,
    args: &Value,
) -> Result<Value, String> {
    if !active_session(app, session) {
        return Err("Remote session is no longer approved".into());
    }
    if cmd == "web_push_test" {
        let subscriber = {
            let _lock = STORE_LOCK.lock().unwrap();
            read_store()?
                .subscribers
                .into_iter()
                .find(|s| s.session == session)
                .ok_or("Enable notifications first")?
        };
        return send(app, subscriber, json!({"title":"Codemux is connected", "body":"Notifications from this desktop are ready.", "tag":"codemux-test"})).await.map(|_| json!(null));
    }
    let _lock = STORE_LOCK.lock().unwrap();
    let mut store = read_store()?;
    match cmd {
        "web_push_config" => {
            let key = VapidSignatureBuilder::from_base64_no_sub(&store.private_key)
                .map_err(|_| "Invalid push key")?;
            Ok(json!({"public_key":URL_SAFE_NO_PAD.encode(key.get_public_key())}))
        }
        "web_push_status" => Ok(store
            .subscribers
            .iter()
            .find(|s| s.session == session)
            .map(|s| json!({"categories":s.categories}))
            .unwrap_or(json!({}))),
        "web_push_subscribe" => {
            let subscription: SubscriptionInfo =
                serde_json::from_value(args["subscription"].clone())
                    .map_err(|_| "Invalid subscription")?;
            if !valid_subscription(&subscription) {
                return Err("Unsupported push endpoint or invalid keys".into());
            }
            let categories = serde_json::from_value(args["categories"].clone())
                .map_err(|_| "Invalid notification preferences")?;
            let host = args["host"]
                .as_str()
                .filter(|s| s.len() <= 256)
                .ok_or("Invalid host")?
                .to_owned();
            store.subscribers.retain(|s| {
                s.session != session && s.subscription.endpoint != subscription.endpoint
            });
            if store.subscribers.len() >= 100 {
                return Err("Too many notification subscriptions".into());
            }
            store.subscribers.push(Subscriber {
                session: session.into(),
                subscription,
                categories,
                host,
            });
            write_store(&store)?;
            Ok(json!(null))
        }
        "web_push_unsubscribe" => {
            store.subscribers.retain(|s| s.session != session);
            write_store(&store)?;
            Ok(json!(null))
        }
        "web_push_preferences" => {
            let categories = serde_json::from_value(args["categories"].clone())
                .map_err(|_| "Invalid notification preferences")?;
            let sub = store
                .subscribers
                .iter_mut()
                .find(|s| s.session == session)
                .ok_or("Enable notifications first")?;
            sub.categories = categories;
            write_store(&store)?;
            Ok(json!(null))
        }
        _ => Err("Unknown push command".into()),
    }
}
async fn send<R: Runtime>(
    app: &AppHandle<R>,
    subscriber: Subscriber,
    mut payload: Value,
) -> Result<(), String> {
    let _permit = SEND_LIMIT
        .get_or_init(|| tokio::sync::Semaphore::new(4))
        .acquire()
        .await
        .map_err(|_| "Push delivery stopped")?;
    if !active_session(app, &subscriber.session) {
        return Err("Remote session is no longer approved".into());
    }
    let shared = app.state::<super::WebRemoteState>().shared();
    {
        let config = shared.config.lock().unwrap();
        if !config.enabled && !config.relay_mode_enabled {
            return Err("Remote access is disabled".into());
        }
    }
    if !valid_subscription(&subscriber.subscription) {
        return Err("Invalid stored subscription".into());
    }
    let key = {
        let _lock = STORE_LOCK.lock().unwrap();
        let store = read_store()?;
        let current = store
            .subscribers
            .iter()
            .find(|s| {
                s.session == subscriber.session
                    && s.subscription.endpoint == subscriber.subscription.endpoint
            })
            .ok_or("Notification subscription was removed")?;
        if let Some(category) = payload["category"].as_str() {
            let enabled = match category {
                "attention" => current.categories.attention,
                "failure" => current.categories.failure,
                _ => current.categories.complete,
            };
            if !enabled {
                return Ok(());
            }
        }
        store.private_key
    };
    payload["host"] = json!(subscriber.host);
    let content = serde_json::to_vec(&payload).map_err(|_| "Invalid notification")?;
    let mut signature = VapidSignatureBuilder::from_base64(&key, &subscriber.subscription)
        .map_err(|_| "Invalid push key")?;
    signature.add_claim("sub", "https://codemux.org");
    let mut builder = WebPushMessageBuilder::new(&subscriber.subscription);
    builder.set_vapid_signature(signature.build().map_err(|_| "Cannot sign notification")?);
    builder.set_payload(ContentEncoding::Aes128Gcm, &content);
    builder.set_ttl(3600);
    let message = builder.build().map_err(|_| "Cannot encrypt notification")?;
    let encrypted = message.payload.ok_or("Missing encrypted payload")?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| "Cannot start push client")?;
    let mut request = client
        .post(&subscriber.subscription.endpoint)
        .header("TTL", "3600")
        .header("Content-Encoding", "aes128gcm")
        .header("Content-Type", "application/octet-stream");
    for (name, value) in encrypted.crypto_headers {
        request = request.header(name, value);
    }
    let response = request
        .body(encrypted.content)
        .send()
        .await
        .map_err(|_| "Push service is unreachable")?;
    if response.status().as_u16() == 404 || response.status().as_u16() == 410 {
        let _lock = STORE_LOCK.lock().unwrap();
        let mut store = read_store()?;
        store
            .subscribers
            .retain(|s| s.subscription.endpoint != subscriber.subscription.endpoint);
        write_store(&store)?;
        return Err("Notification subscription expired. Enable notifications again.".into());
    }
    if !response.status().is_success() {
        return Err(format!(
            "Push service returned {}",
            response.status().as_u16()
        ));
    }
    Ok(())
}
fn matching_pane(node: &PaneNodeSnapshot, source: &str) -> Option<String> {
    match node {
        PaneNodeSnapshot::Terminal {
            pane_id,
            session_id,
            ..
        } if session_id.0 == source => Some(pane_id.0.clone()),
        PaneNodeSnapshot::AgentChat {
            pane_id, thread_id, ..
        } if thread_id.as_deref() == Some(source) => Some(pane_id.0.clone()),
        PaneNodeSnapshot::Split { children, .. } => {
            children.iter().find_map(|c| matching_pane(c, source))
        }
        _ => None,
    }
}
/// Called before desktop-focused Review→Idle suppression, for CLI and GUI agents.
pub fn agent_status<R: Runtime>(app: &AppHandle<R>, source: &str, status: PaneStatus) {
    let category = match status {
        PaneStatus::Permission => "attention",
        PaneStatus::Review => "complete",
        PaneStatus::Working | PaneStatus::Monitoring => "working",
        _ => return,
    };
    agent_event(app, source, category);
}
pub fn agent_failure<R: Runtime>(app: &AppHandle<R>, source: &str) {
    agent_event(app, source, "failure");
}
fn should_deliver(
    last: &mut HashMap<String, (String, Instant)>,
    source: &str,
    category: &str,
) -> bool {
    // Bound the table when hosts run many short-lived sessions.
    if last.len() >= 4096 && !last.contains_key(source) {
        if let Some(oldest) = last
            .iter()
            .min_by_key(|(_, (_, at))| *at)
            .map(|(source, _)| source.clone())
        {
            last.remove(&oldest);
        }
    }
    // A failed turn can subsequently settle to Review; only a new working
    // transition starts another successful run.
    if category == "complete" && last.get(source).is_some_and(|(c, _)| c == "failure") {
        return false;
    }
    let previous = last.insert(source.into(), (category.into(), Instant::now()));
    if category == "working"
        || previous
            .as_ref()
            .is_some_and(|(c, at)| c == category && at.elapsed() < Duration::from_secs(60))
    {
        return false;
    }
    true
}
fn agent_event<R: Runtime>(app: &AppHandle<R>, source: &str, category: &'static str) {
    if !should_deliver(
        &mut LAST_EVENT.get_or_init(Default::default).lock().unwrap(),
        source,
        category,
    ) {
        return;
    }
    let snapshot = app.state::<AppStateStore>().snapshot();
    let Some((workspace, pane)) = snapshot.workspaces.iter().find_map(|w| {
        w.surfaces
            .iter()
            .find_map(|s| matching_pane(&s.root, source))
            .map(|p| (w, p))
    }) else {
        return;
    };
    if workspace.notifications_muted {
        return;
    }
    let payload = json!({"category":category, "title": match category {"attention" => "Codemux needs you", "failure" => "Agent failed", _ => "Ready for review"}, "body":workspace.title.chars().take(160).collect::<String>(), "workspace_id":workspace.workspace_id.0, "pane_id":pane, "tag":format!("codemux-{}-{source}-{category}", workspace.workspace_id.0)});
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let subscribers = {
            let _lock = STORE_LOCK.lock().unwrap();
            // Don't create a key or file until a user actually enables push.
            if !store_path().is_ok_and(|p| p.exists()) {
                return;
            }
            let Ok(mut store) = read_store() else {
                return;
            };
            let before = store.subscribers.len();
            store
                .subscribers
                .retain(|s| active_session(&app, &s.session));
            if before != store.subscribers.len() {
                let _ = write_store(&store);
            }
            store.subscribers
        };
        for subscriber in subscribers {
            let enabled = match category {
                "attention" => subscriber.categories.attention,
                "failure" => subscriber.categories.failure,
                _ => subscriber.categories.complete,
            };
            if enabled {
                if let Err(error) = send(&app, subscriber, payload.clone()).await {
                    eprintln!("[web-push] {error}");
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_turns_do_not_also_announce_success_and_new_runs_reset() {
        let mut last = HashMap::new();
        assert!(!should_deliver(&mut last, "thread", "working"));
        assert!(should_deliver(&mut last, "thread", "attention"));
        assert!(!should_deliver(&mut last, "thread", "attention"));
        assert!(should_deliver(&mut last, "thread", "failure"));
        assert!(!should_deliver(&mut last, "thread", "complete"));
        assert!(!should_deliver(&mut last, "thread", "working"));
        assert!(should_deliver(&mut last, "thread", "complete"));
        assert!(!should_deliver(&mut last, "thread", "complete"));
    }
    #[test]
    fn subscription_keys_must_be_valid_sizes() {
        let sub = SubscriptionInfo::new("https://web.push.apple.com/test", "BA", "AA");
        assert!(!valid_subscription(&sub));
    }
    #[test]
    fn endpoints_reject_ssrf_and_lookalikes() {
        for bad in [
            "http://fcm.googleapis.com/a",
            "https://127.0.0.1/",
            "https://fcm.googleapis.com.evil.test/a",
            "https://evilpush.apple.com/a",
            "https://user@fcm.googleapis.com/a",
            "https://fcm.googleapis.com:8443/a",
        ] {
            assert!(!valid_endpoint(bad), "{bad}");
        }
        for good in [
            "https://fcm.googleapis.com/fcm/send/test",
            "https://web.push.apple.com/test",
            "https://updates.push.services.mozilla.com/wpush/v2/test",
        ] {
            assert!(valid_endpoint(good));
        }
    }
}
