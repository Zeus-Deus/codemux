use super::{
    credentials::Credentials,
    git::Git,
    http::Http,
    permissions::{Contexts, Grant, Source},
    protocol::{Event, Host},
    storage::Storage,
    workspace::Workspace,
    ErrorCode, Manifest, ProtocolError, Result,
};
use codemux_addon_protocol::{limits, manifest::Permission, ui::Tree};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex as StdMutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::{broadcast, oneshot, Mutex, Semaphore};
use tokio_util::sync::CancellationToken;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    InstalledDisabled,
    Activating,
    EnabledIdle,
    EnabledRunning,
    Updating,
    FailedDisabled,
    IncompatibleDisabled,
    BlockedDisabled,
    Removing,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Installation {
    pub installation_id: String,
    pub manifest: Manifest,
    pub source: Source,
    pub digest: String,
    pub desired_enabled: bool,
    pub status: Status,
    pub data_generation: String,
    pub grant: Option<Grant>,
    pub failure: Option<String>,
    #[serde(default)]
    pub previous: Option<super::lifecycle::Previous>,
}
impl Installation {
    pub(super) fn validate_record(&self) -> Result<()> {
        fn uuid(value: &str) -> bool {
            uuid::Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value)
        }
        fn digest(value: &str) -> bool {
            value.len() == 64
                && value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }
        if !uuid(&self.installation_id) || !uuid(&self.data_generation) || !digest(&self.digest) {
            return Err(ProtocolError::new(
                ErrorCode::StorageUnavailable,
                "Invalid add-on registry identity; plugins are paused",
            ));
        }
        self.manifest.validate(None)?;
        if let Some(previous) = &self.previous {
            if !uuid(&previous.data_generation)
                || !digest(&previous.digest)
                || previous.manifest.id != self.manifest.id
                || previous.source != self.source
            {
                return Err(ProtocolError::new(
                    ErrorCode::StorageUnavailable,
                    "Invalid add-on recovery snapshot",
                ));
            }
            previous.manifest.validate(None)?;
        }
        Ok(())
    }
}
#[derive(Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum UiEvent {
    Inventory,
    DevelopmentReview {
        review: super::lifecycle::Review,
    },
    DevelopmentError {
        message: String,
    },
    Tree {
        plugin_id: String,
        generation: String,
        view_id: String,
        revision: u64,
        tree: Tree,
    },
    Effect {
        plugin_id: String,
        generation: String,
        request_id: String,
        operation: String,
        params: Value,
    },
    Stopped {
        plugin_id: String,
        generation: String,
        message: String,
    },
}
struct View {
    tree: Tree,
    context: String,
    revision: u64,
    acknowledged: u64,
}
pub struct Running {
    pub host: Host,
    pub manifest: Manifest,
    pub installation_id: String,
    pub probe: bool,
    activated: AtomicBool,
    cancel: CancellationToken,
    stopped: CancellationToken,
    storage: StdMutex<Option<Storage>>,
    settings: StdMutex<Value>,
    views: Mutex<HashMap<String, View>>,
    // Views are numbered per generation, so a late batch for any disposed
    // view is recognized without retaining one tombstone per disposal.
    issued_views: AtomicU64,
    ui_rate: Mutex<limits::RateLimit>,
    requests: Arc<Semaphore>,
    git: Git,
    http: Http,
    last_used: Mutex<Instant>,
    notifications: Mutex<limits::RateLimit>,
    logs: Mutex<VecDeque<String>>,
}
impl Running {
    pub fn generation(&self) -> &str {
        &self.host.generation
    }
    fn issued_view(&self, view_id: &str) -> bool {
        view_id
            .strip_prefix("view-")
            .and_then(|n| n.parse::<u64>().ok())
            .is_some_and(|n| {
                format!("view-{n}") == view_id && n <= self.issued_views.load(Ordering::Acquire)
            })
    }
}
struct PendingEffect {
    generation: String,
    sender: oneshot::Sender<Result<Value>>,
    cancel: CancellationToken,
    deadline: Instant,
    claimed: Option<Instant>,
}
/// A claimed effect is being applied by the trusted UI. Its result may arrive
/// shortly after the interaction deadline, which only bounds claiming.
const CLAIMED_GRACE: Duration = Duration::from_secs(2);
impl PendingEffect {
    fn expires(&self) -> Instant {
        self.claimed
            .map_or(self.deadline, |at| self.deadline.max(at + CLAIMED_GRACE))
    }
}
pub struct Manager {
    pub root: PathBuf,
    host_path: PathBuf,
    pub(super) registry: StdMutex<Connection>,
    pub credentials: Credentials,
    pub contexts: Mutex<Contexts>,
    pub(super) catalog_serial: Mutex<()>,
    running: Arc<Mutex<HashMap<String, Arc<Running>>>>,
    operations: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    effects: Mutex<HashMap<String, PendingEffect>>,
    pub events: broadcast::Sender<UiEvent>,
    paused: AtomicBool,
}
fn storage_error(_: rusqlite::Error) -> ProtocolError {
    ProtocolError::new(
        ErrorCode::StorageUnavailable,
        "Add-on registry is unavailable; plugins are paused",
    )
}
impl Manager {
    pub fn open(root: PathBuf, host_path: PathBuf) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&root).map_err(|_| {
            ProtocolError::new(
                ErrorCode::StorageUnavailable,
                "Cannot create add-on storage",
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).map_err(
                |_| {
                    ProtocolError::new(
                        ErrorCode::StorageUnavailable,
                        "Cannot protect add-on storage",
                    )
                },
            )?;
        }
        let registry = Connection::open(root.join("registry.sqlite")).map_err(storage_error)?;
        registry.execute_batch("PRAGMA journal_mode=WAL;PRAGMA synchronous=FULL;CREATE TABLE IF NOT EXISTS installations(id TEXT PRIMARY KEY,plugin_id TEXT NOT NULL UNIQUE,record TEXT NOT NULL);CREATE TABLE IF NOT EXISTS settings(installation TEXT PRIMARY KEY,value TEXT NOT NULL);CREATE TABLE IF NOT EXISTS metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL);CREATE TABLE IF NOT EXISTS credential_entries(installation TEXT NOT NULL,id TEXT NOT NULL,PRIMARY KEY(installation,id));CREATE TABLE IF NOT EXISTS file_cleanup(installation TEXT PRIMARY KEY,record TEXT NOT NULL);CREATE TABLE IF NOT EXISTS cleanup(installation TEXT NOT NULL,credential TEXT NOT NULL,PRIMARY KEY(installation,credential));").map_err(storage_error)?;
        let configured_credentials = {
            let mut query = registry
                .prepare("SELECT installation,id FROM credential_entries")
                .map_err(storage_error)?;
            let entries = query
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map_err(storage_error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(storage_error)?;
            if entries.iter().any(|(installation, id)| {
                uuid::Uuid::parse_str(installation).is_err()
                    || !codemux_addon_protocol::catalog::hex(id, 64)
            }) {
                return Err(ProtocolError::invalid("Invalid credential index"));
            }
            entries
        };
        let (events, _) = broadcast::channel(32);
        let stored_paused = registry
            .query_row("SELECT value FROM metadata WHERE key='paused'", [], |row| {
                row.get::<_, String>(0)
            })
            .ok()
            .is_some_and(|v| v == "true");
        let manager = Arc::new(Self {
            root,
            host_path,
            registry: StdMutex::new(registry),
            credentials: Credentials::with_configured(configured_credentials),
            contexts: Mutex::new(Contexts::default()),
            catalog_serial: Mutex::new(()),
            running: Arc::new(Mutex::new(HashMap::new())),
            operations: Mutex::new(HashMap::new()),
            effects: Mutex::new(HashMap::new()),
            events,
            paused: AtomicBool::new(
                stored_paused
                    || std::env::var_os("CODEMUX_DISABLE_ADDONS").is_some_and(|v| v == "1"),
            ),
        });
        manager.recover()?;
        manager.recover_session()?;
        manager.prune_unreferenced()?;
        Ok(manager)
    }
    pub(crate) fn record_credential(&self, installation: &str, id: &str) -> Result<()> {
        self.registry
            .lock()
            .unwrap()
            .execute(
                "INSERT OR IGNORE INTO credential_entries(installation,id) VALUES(?1,?2)",
                params![installation, id],
            )
            .map_err(storage_error)?;
        Ok(())
    }
    pub fn paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }
    pub fn list(&self) -> Result<Vec<Installation>> {
        let connection = self.registry.lock().unwrap();
        let mut query = connection
            .prepare("SELECT record FROM installations ORDER BY plugin_id")
            .map_err(storage_error)?;
        let records = query
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(storage_error)?;
        let mut list = Vec::new();
        for record in records {
            let record: Installation = serde_json::from_str(&record.map_err(storage_error)?)
                .map_err(|_| {
                    ProtocolError::new(
                        ErrorCode::StorageUnavailable,
                        "Add-on registry is corrupt; plugins are paused",
                    )
                })?;
            record.validate_record()?;
            list.push(record);
        }
        Ok(list)
    }
    pub fn installation(&self, id: &str) -> Result<Installation> {
        self.list()?
            .into_iter()
            .find(|i| i.manifest.id == id)
            .ok_or_else(|| ProtocolError::new(ErrorCode::PluginStopped, "Add-on is not installed"))
    }
    pub(crate) fn save(&self, installation: &Installation) -> Result<()> {
        installation.validate_record()?;
        let mut connection = self.registry.lock().unwrap();
        let transaction = connection.transaction().map_err(storage_error)?;
        transaction.execute("INSERT INTO installations(id,plugin_id,record) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET record=excluded.record",params![installation.installation_id,installation.manifest.id,serde_json::to_string(installation).unwrap()]).map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        let _ = self.events.send(UiEvent::Inventory);
        Ok(())
    }
    pub async fn operation(&self, id: &str) -> Arc<Mutex<()>> {
        self.operations
            .lock()
            .await
            .entry(id.into())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
    pub fn settings(&self, installation: &Installation) -> Result<Value> {
        use rusqlite::OptionalExtension;
        let raw: Option<String> = self
            .registry
            .lock()
            .unwrap()
            .query_row(
                "SELECT value FROM settings WHERE installation=?1",
                [&installation.installation_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage_error)?;
        let saved: Value = raw
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(json!({}));
        let mut settings = serde_json::Map::new();
        for declaration in &installation.manifest.settings {
            let value = saved
                .get(declaration.id())
                .filter(|v| declaration.accepts(v))
                .cloned()
                .unwrap_or_else(|| declaration.default_value());
            settings.insert(declaration.id().into(), value);
        }
        Ok(settings.into())
    }
    pub async fn set_settings(&self, id: &str, value: Value) -> Result<()> {
        let operation = self.operation(id).await;
        let _lock = operation.lock().await;
        let installation = self.installation(id)?;
        let settings = value
            .as_object()
            .ok_or_else(|| ProtocolError::invalid("Invalid settings"))?;
        if settings.len() != installation.manifest.settings.len()
            || installation
                .manifest
                .settings
                .iter()
                .any(|s| !settings.get(s.id()).is_some_and(|v| s.accepts(v)))
        {
            return Err(ProtocolError::invalid("Settings do not match this package"));
        }
        self.registry.lock().unwrap().execute("INSERT INTO settings(installation,value) VALUES(?1,?2) ON CONFLICT(installation) DO UPDATE SET value=excluded.value",params![installation.installation_id,serde_json::to_string(&value).unwrap()]).map_err(storage_error)?;
        let running = self.running.lock().await.get(id).cloned();
        if let Some(running) = running {
            *running.settings.lock().unwrap() = value.clone();
            running
                .host
                .send("settings.changed", json!({"settings":value}))
                .await?;
        }
        Ok(())
    }
    // Source is supplied only by the package validator, not by a Tauri command.
    pub(crate) async fn activate(
        self: &Arc<Self>,
        installation: Installation,
        source: String,
        probe: bool,
    ) -> Result<Arc<Running>> {
        self.check_blocklist(&installation.manifest.id, &installation.digest)?;
        if self.paused()
            || (!installation.desired_enabled && !probe)
            || (!probe
                && matches!(
                    installation.status,
                    Status::FailedDisabled
                        | Status::BlockedDisabled
                        | Status::IncompatibleDisabled
                        | Status::Removing
                ))
        {
            return Err(ProtocolError::new(
                ErrorCode::PluginStopped,
                "Add-on is disabled or paused",
            ));
        }
        installation
            .grant
            .as_ref()
            .ok_or_else(|| {
                ProtocolError::new(ErrorCode::PermissionDenied, "Permission review required")
            })?
            .check(
                &installation.installation_id,
                &installation.manifest,
                &installation.source,
                &installation.digest,
            )?;
        let mut hosts = self.running.lock().await;
        if let Some(running) = hosts.get(&installation.manifest.id) {
            return Ok(running.clone());
        }
        if hosts.len() >= 8 {
            let idle = hosts
                .iter()
                .find(|(_, r)| {
                    r.requests.available_permits() == 16
                        && r.views.try_lock().is_ok_and(|views| views.is_empty())
                })
                .map(|(id, _)| id.clone());
            if let Some(id) = idle {
                drop(hosts);
                self.stop(&id, None).await;
                hosts = self.running.lock().await;
            }
        }
        if hosts.len() >= 8 {
            return Err(ProtocolError::new(
                ErrorCode::ResourceLimit,
                "Eight add-ons are already active; close an idle view and retry",
            ));
        }
        let state = self
            .root
            .join("state")
            .join(&installation.installation_id)
            .join(&installation.data_generation);
        std::fs::create_dir_all(&state).map_err(|_| {
            ProtocolError::new(ErrorCode::StorageUnavailable, "Cannot open plugin state")
        })?;
        let storage = Storage::open(&state.join("state.sqlite"))?;
        let settings = self.settings(&installation)?;
        self.registry
            .lock()
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO metadata(key,value) VALUES(?1,'pending')",
                [format!("activation:{}", installation.manifest.id)],
            )
            .map_err(storage_error)?;
        let (host, mut events) =
            Host::spawn(&self.host_path, &installation.manifest, &source).await?;
        let running = Arc::new(Running {
            host,
            manifest: installation.manifest.clone(),
            installation_id: installation.installation_id.clone(),
            probe,
            activated: AtomicBool::new(false),
            cancel: CancellationToken::new(),
            stopped: CancellationToken::new(),
            storage: StdMutex::new(Some(storage)),
            settings: StdMutex::new(settings),
            views: Mutex::new(HashMap::new()),
            issued_views: AtomicU64::new(0),
            ui_rate: Mutex::new(limits::RateLimit::default()),
            requests: Arc::new(Semaphore::new(16)),
            git: Git::default(),
            http: Http::default(),
            last_used: Mutex::new(Instant::now()),
            notifications: Mutex::new(limits::RateLimit::default()),
            logs: Mutex::new(VecDeque::new()),
        });
        hosts.insert(installation.manifest.id.clone(), running.clone());
        drop(hosts);
        let (ready, readiness) = oneshot::channel();
        let manager = self.clone();
        let instance = running.clone();
        tokio::spawn(async move {
            let mut ready = Some(ready);
            loop {
                let idle_at = *instance.last_used.lock().await + Duration::from_secs(60);
                let can_idle = instance.views.lock().await.is_empty()
                    && instance.requests.available_permits() == 16;
                let idle = async {
                    if can_idle {
                        tokio::time::sleep_until(idle_at.into()).await
                    } else {
                        std::future::pending::<()>().await
                    }
                };
                tokio::select! {
                 _=instance.cancel.cancelled()=>break,
                 _=idle=>{if instance.views.lock().await.is_empty()&&instance.requests.available_permits()==16&&instance.last_used.lock().await.elapsed()>=Duration::from_secs(60){manager.stop(&instance.manifest.id,None).await;break}},
                 event=events.recv()=>{let result=match event{
                  Some(Event::Message(message))=>{
                   if message.method.as_deref()==Some("ready")&&message.params.as_ref().is_some_and(|p|p["phase"]=="activated"){
                    let declarations=&instance.manifest.contributes;let mut expected=Vec::new();for c in &declarations.commands{expected.push(format!("commands/{}",c.id))}for (kind,views) in [("panels",&declarations.panels),("composerActions",&declarations.composer_actions),("composerViews",&declarations.composer_views)]{for v in views{expected.push(format!("{kind}/{}",v.id))}}expected.sort();
                    let mut actual:Vec<String>=serde_json::from_value(message.params.unwrap()["registrations"].clone()).unwrap_or_default();actual.sort();
                    if actual!=expected{Err(ProtocolError::invalid("Plugin registration does not match its manifest"))}else{instance.activated.store(true,Ordering::Release);if let Some(ready)=ready.take(){let _=ready.send(());}Ok(())}
                   }else{manager.message(&instance,message).await}
                  },Some(Event::Stopped(error))=>Err(error),None=>Err(ProtocolError::new(ErrorCode::PluginStopped,"Plugin host exited"))};
                  if let Err(error)=result{manager.stop(&instance.manifest.id,Some(error.message)).await;break}
                 }
                }
            }
        });
        if let Err(error) = running.host.send("activate", json!({})).await {
            self.stop(&installation.manifest.id, Some(error.message.clone()))
                .await;
            return Err(error);
        }
        if !matches!(
            tokio::time::timeout(Duration::from_secs(2), readiness).await,
            Ok(Ok(()))
        ) {
            self.stop(
                &installation.manifest.id,
                Some("Plugin did not activate".into()),
            )
            .await;
            return Err(ProtocolError::new(
                ErrorCode::Timeout,
                "Plugin did not activate",
            ));
        }
        if self.paused() || running.cancel.is_cancelled() {
            self.stop(&installation.manifest.id, None).await;
            return Err(ProtocolError::new(
                ErrorCode::PluginStopped,
                "Add-on was paused during activation",
            ));
        }
        self.registry
            .lock()
            .unwrap()
            .execute(
                "DELETE FROM metadata WHERE key=?1",
                [format!("activation:{}", installation.manifest.id)],
            )
            .map_err(storage_error)?;
        if !probe {
            let mut installation = installation;
            installation.status = Status::EnabledRunning;
            self.save(&installation)?;
        }
        Ok(running)
    }
    async fn message(
        self: &Arc<Self>,
        running: &Arc<Running>,
        message: codemux_addon_protocol::wire::Envelope,
    ) -> Result<()> {
        if running.cancel.is_cancelled() {
            return Err(ProtocolError::new(
                ErrorCode::PluginStopped,
                "Plugin stopped",
            ));
        }
        let params = message.params.unwrap_or(json!({}));
        match message.method.as_deref() {
            Some("host.request") => {
                let id = message
                    .id
                    .ok_or_else(|| ProtocolError::invalid("Host operation requires an ID"))?;
                let permit = running.requests.clone().try_acquire_owned().map_err(|_| {
                    ProtocolError::new(ErrorCode::ResourceLimit, "Too many host requests")
                })?;
                let manager = self.clone();
                let instance = running.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let operation = params["operation"].as_str().unwrap_or("");
                    let timeout = if operation == "http.fetch" { 30 } else { 15 };
                    let result = tokio::select! {_ = instance.cancel.cancelled()=>Err(ProtocolError::new(ErrorCode::PluginStopped,"Plugin stopped")),result=tokio::time::timeout(Duration::from_secs(timeout),manager.request(&instance,operation,params["params"].clone()))=>result.unwrap_or_else(|_|Err(ProtocolError::new(ErrorCode::Timeout,"Host request timed out")))};
                    // Never leave a request unanswered because its result
                    // does not fit a frame; the plugin would wait for TIMEOUT.
                    if let Err(error) = instance.host.respond(id, result).await {
                        if error.data.code == ErrorCode::ResourceLimit {
                            let _ = instance
                                .host
                                .respond(
                                    id,
                                    Err(ProtocolError::new(
                                        ErrorCode::ResourceLimit,
                                        "Host response exceeds the 1 MiB message limit",
                                    )),
                                )
                                .await;
                        }
                    }
                });
                Ok(())
            }
            Some("ui.patch") => {
                let view_id = params["viewId"]
                    .as_str()
                    .ok_or_else(|| ProtocolError::invalid("Missing view"))?;
                let records = params["records"]
                    .as_array()
                    .ok_or_else(|| ProtocolError::invalid("Missing mutation batch"))?;
                // The child may flush a batch before it receives view.unmount.
                // Ignore late batches for disposed views; reject unknown IDs.
                if !running.views.lock().await.contains_key(view_id) {
                    return if running.issued_view(view_id) {
                        Ok(())
                    } else {
                        Err(ProtocolError::invalid("Unknown plugin view"))
                    };
                }
                if !running
                    .ui_rate
                    .lock()
                    .await
                    .accept(Instant::now(), 30, 1800)
                {
                    return Err(ProtocolError::new(
                        ErrorCode::ResourceLimit,
                        "UI traffic limit",
                    ));
                }
                let mut views = running.views.lock().await;
                // Live callbacks are bounded per plugin, not only per view.
                let others: usize = views
                    .iter()
                    .filter(|(id, _)| id.as_str() != view_id)
                    .map(|(_, v)| v.tree.callback_count())
                    .sum();
                let Some(view) = views.get_mut(view_id) else {
                    return Ok(());
                };
                if view.revision - view.acknowledged >= 2 {
                    return Err(ProtocolError::new(
                        ErrorCode::ResourceLimit,
                        "UI update queue overflow",
                    ));
                }
                view.tree
                    .apply_within(records, limits::CALLBACKS.saturating_sub(others))?;
                view.revision += 1;
                let _ = self.events.send(UiEvent::Tree {
                    plugin_id: running.manifest.id.clone(),
                    generation: running.generation().into(),
                    view_id: view_id.into(),
                    revision: view.revision,
                    tree: view.tree.clone(),
                });
                Ok(())
            }
            Some("log") => {
                // Plugin-supplied text may contain service response bodies. Keep
                // only an attributed diagnostic count, never raw author output.
                let message = format!(
                    "Plugin log received ({} bytes; content omitted)",
                    params["message"].as_str().map_or(0, |s| s.len().min(1024))
                );
                let mut logs = running.logs.lock().await;
                logs.push_back(message);
                while logs.iter().map(String::len).sum::<usize>() > 65536 {
                    logs.pop_front();
                }
                Ok(())
            }
            Some("ready") => Ok(()),
            _ => Err(ProtocolError::invalid("Unknown child method")),
        }
    }
    pub async fn stop(&self, id: &str, failure: Option<String>) {
        // Keep the generation registered until its child has been reaped. This
        // prevents a lazy activation from overlapping a generation being stopped.
        let running = {
            let hosts = self.running.lock().await;
            match hosts.get(id) {
                Some(running) if !running.cancel.is_cancelled() => {
                    running.cancel.cancel();
                    Some(running.clone())
                }
                Some(running) => {
                    let stopped = running.stopped.clone();
                    drop(hosts);
                    let _ = tokio::time::timeout(Duration::from_secs(2), stopped.cancelled()).await;
                    return;
                }
                None => None,
            }
        };
        let probe = running.as_ref().is_some_and(|r| r.probe);
        if let Some(running) = &running {
            // Cancelled broker tasks and activation callers may still hold an
            // Arc<Running>. Release SQLite explicitly instead of retaining its
            // Windows file locks until those references happen to disappear.
            // The same mutex serializes any already-entered storage operation.
            running.storage.lock().unwrap().take();
            self.contexts
                .lock()
                .await
                .revoke_generation(running.generation());
            self.effects
                .lock()
                .await
                .retain(|_, effect| effect.generation != running.generation());
            let _ = self.events.send(UiEvent::Stopped {
                plugin_id: id.into(),
                generation: running.generation().into(),
                message: failure.clone().unwrap_or_else(|| "Plugin stopped".into()),
            });
            if !running.host.stop().await {
                // A kernel-level termination delay must never make room for a
                // second generation. Keep the cancelled instance registered
                // until the supervisor confirms actual reaping.
                if let Ok(mut installation) = self.installation(id) {
                    installation.status = Status::FailedDisabled;
                    installation.failure =
                        Some("The plugin process has not exited; it remains quarantined".into());
                    let _ = self.save(&installation);
                }
                let hosts = self.running.clone();
                let instance = running.clone();
                let id = id.to_owned();
                tokio::spawn(async move {
                    instance.host.reaped().await;
                    let mut hosts = hosts.lock().await;
                    if hosts
                        .get(&id)
                        .is_some_and(|r| r.generation() == instance.generation())
                    {
                        hosts.remove(&id);
                    }
                    instance.stopped.cancel();
                });
                return;
            }
        }
        let _ = self.registry.lock().unwrap().execute(
            "DELETE FROM metadata WHERE key=?1",
            [format!("activation:{id}")],
        );
        if probe {
            self.running.lock().await.remove(id);
            if let Some(running) = running {
                running.stopped.cancel();
            }
            return;
        }
        if let Ok(mut installation) = self.installation(id) {
            installation.status = if failure.is_some() {
                Status::FailedDisabled
            } else if matches!(
                installation.status,
                Status::BlockedDisabled | Status::IncompatibleDisabled | Status::Removing
            ) {
                installation.status
            } else if installation.desired_enabled {
                Status::EnabledIdle
            } else {
                Status::InstalledDisabled
            };
            if failure.is_some()
                || !matches!(
                    installation.status,
                    Status::BlockedDisabled | Status::IncompatibleDisabled
                )
            {
                installation.failure = failure;
            }
            let _ = self.save(&installation);
        }
        if let Some(running) = running {
            self.running.lock().await.remove(id);
            running.stopped.cancel();
        }
    }
    pub async fn ensure_active(self: &Arc<Self>, id: &str) -> Result<Arc<Running>> {
        let operation = self.operation(id).await;
        let _lock = operation.lock().await;
        if let Some(running) = self.running.lock().await.get(id).cloned() {
            if running.cancel.is_cancelled() {
                return Err(ProtocolError::new(
                    ErrorCode::PluginStopped,
                    "Add-on is stopping; retry shortly",
                ));
            }
            return Ok(running);
        }
        let installation = self.installation(id)?;
        let path = self
            .root
            .join("packages")
            .join(id)
            .join(&installation.digest)
            .join("package.cmxaddon");
        let digest = installation.digest.clone();
        let package = tokio::task::spawn_blocking(move || {
            super::package::Package::read(&path, Some(&digest))
        })
        .await
        .map_err(|_| ProtocolError::invalid("Package verification failed"))??;
        if serde_json::to_value(&package.manifest).unwrap()
            != serde_json::to_value(&installation.manifest).unwrap()
        {
            return Err(ProtocolError::invalid("Installed manifest was modified"));
        }
        self.activate(installation, package.source(), false).await
    }
    pub async fn unmount(&self, running: &Running, view_id: &str) -> Result<()> {
        let Some(view) = running.views.lock().await.remove(view_id) else {
            return Ok(());
        };
        self.contexts.lock().await.revoke(&view.context);
        running
            .host
            .send("view.unmount", json!({"viewId":view_id}))
            .await?;
        *running.last_used.lock().await = Instant::now();
        Ok(())
    }
    pub async fn shutdown(&self) {
        self.paused.store(true, Ordering::Release);
        let ids: Vec<_> = self.running.lock().await.keys().cloned().collect();
        futures_util::future::join_all(ids.iter().map(|id| self.stop(id, None))).await;
    }
    pub fn resume(&self) -> Result<()> {
        if std::env::var_os("CODEMUX_DISABLE_ADDONS").is_some_and(|v| v == "1") {
            return Err(ProtocolError::new(
                ErrorCode::PluginStopped,
                "Restart without --disable-addons or CODEMUX_DISABLE_ADDONS to enable plugins",
            ));
        }
        self.registry
            .lock()
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO metadata(key,value) VALUES('paused','false')",
                [],
            )
            .map_err(storage_error)?;
        self.paused.store(false, Ordering::Release);
        let _ = self.events.send(UiEvent::Inventory);
        Ok(())
    }
    fn recover_session(&self) -> Result<()> {
        let pending: bool = self
            .registry
            .lock()
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM metadata WHERE key LIKE 'activation:%')",
                [],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        if pending {
            self.registry.lock().unwrap().execute_batch("INSERT OR REPLACE INTO metadata(key,value) VALUES('paused','true'); DELETE FROM metadata WHERE key LIKE 'activation:%';").map_err(storage_error)?;
            self.paused.store(true, Ordering::Release);
        }
        for mut installation in self.list()? {
            if matches!(
                installation.status,
                Status::Activating | Status::EnabledRunning | Status::Updating
            ) {
                installation.status = if installation.desired_enabled {
                    Status::EnabledIdle
                } else {
                    Status::InstalledDisabled
                };
                if pending {
                    installation.failure=Some("The app stopped before an add-on activation checkpoint. All add-ons are paused; review them before resuming.".into());
                }
                self.save(&installation)?;
            }
        }
        Ok(())
    }
    pub async fn pause_all(&self) {
        let _ = self.registry.lock().unwrap().execute(
            "INSERT OR REPLACE INTO metadata(key,value) VALUES('paused','true')",
            [],
        );
        self.paused.store(true, Ordering::Release);
        let ids: Vec<_> = self.running.lock().await.keys().cloned().collect();
        futures_util::future::join_all(ids.iter().map(|id| self.stop(id, None))).await;
        let _ = self.events.send(UiEvent::Inventory);
    }
    pub async fn change_workspace(&self, workspace: Option<Workspace>) {
        self.contexts.lock().await.change_workspace();
        let hosts: Vec<_> = self.running.lock().await.values().cloned().collect();
        for running in hosts {
            running.views.lock().await.clear();
            // Every plugin is told so it unmounts its views, but only a
            // workspace.read grant receives a context for the new project.
            let context = match &workspace {
                Some(workspace)
                    if running
                        .manifest
                        .permissions
                        .contains(&Permission::WorkspaceRead) =>
                {
                    self.contexts
                        .lock()
                        .await
                        .issue(running.generation(), Some(workspace.clone()), None)
                        .ok()
                }
                _ => None,
            };
            let _ = running
                .host
                .send("workspace.changed", json!({ "context": context }))
                .await;
        }
    }
    async fn context(
        &self,
        running: &Running,
        params: &Value,
    ) -> Result<super::permissions::Context> {
        let handle = params["context"].as_str().ok_or_else(|| {
            ProtocolError::new(ErrorCode::ContextStale, "Context handle required")
        })?;
        self.contexts
            .lock()
            .await
            .get(handle, running.generation())
            .cloned()
    }
    async fn request(&self, running: &Running, operation: &str, params: Value) -> Result<Value> {
        if !params.is_object() {
            return Err(ProtocolError::invalid(
                "Operation parameters must be an object",
            ));
        }
        // No generic invoke and no author-selected paths or plugin identity.
        let allowed: &[&str] = match operation {
            "workspace.current" | "git.summary" => &["context"],
            "settings.get" => &[],
            "storage.get" | "storage.delete" => &["scope", "context", "key"],
            "storage.set" => &["scope", "context", "key", "value"],
            "http.fetch" => &["context", "origin", "path", "method", "headers", "body"],
            "composer.appendText" => &["context", "text"],
            "panels.open" | "composerViews.open" => &["id", "context"],
            "links.open" => &["url", "context"],
            "ui.notify" => &["message"],
            _ => return Err(ProtocolError::invalid("Unknown SDK operation")),
        };
        if params
            .as_object()
            .unwrap()
            .keys()
            .any(|key| !allowed.contains(&key.as_str()))
        {
            return Err(ProtocolError::invalid("Unknown operation parameter"));
        }
        match operation {
            "workspace.current" => {
                super::permissions::require(&running.manifest, Permission::WorkspaceRead)?;
                Ok(serde_json::to_value(self.context(running, &params).await?.workspace).unwrap())
            }
            "git.summary" => {
                super::permissions::require(&running.manifest, Permission::GitRead)?;
                let context = self.context(running, &params).await?;
                let workspace = context.workspace.clone().ok_or_else(|| {
                    ProtocolError::new(ErrorCode::NoWorkspace, "No workspace is open")
                })?;
                let summary = running.git.summary(&workspace, &context.cancel).await?;
                self.contexts.lock().await.live(&context)?;
                Ok(serde_json::to_value(summary).unwrap())
            }
            "settings.get" => Ok(running.settings.lock().unwrap().clone()),
            "storage.get" | "storage.set" | "storage.delete" => {
                let scope = match params["scope"].as_str() {
                    Some("global") => "global".into(),
                    Some("workspace") => {
                        let context = self.context(running, &params).await?;
                        super::workspace::storage_scope(&context.workspace.ok_or_else(|| {
                            ProtocolError::new(ErrorCode::NoWorkspace, "No workspace is open")
                        })?)
                    }
                    _ => return Err(ProtocolError::invalid("Invalid storage scope")),
                };
                let key = params["key"]
                    .as_str()
                    .ok_or_else(|| ProtocolError::invalid("Missing storage key"))?;
                let mut storage = running.storage.lock().unwrap();
                let storage = storage.as_mut().ok_or_else(|| {
                    ProtocolError::new(ErrorCode::PluginStopped, "Plugin storage is closed")
                })?;
                match operation {
                    "storage.get" => storage.get(&scope, key),
                    "storage.set" => {
                        storage.set(
                            &scope,
                            key,
                            params
                                .get("value")
                                .ok_or_else(|| ProtocolError::invalid("Missing storage value"))?,
                        )?;
                        Ok(Value::Null)
                    }
                    _ => {
                        storage.delete(&scope, key)?;
                        Ok(Value::Null)
                    }
                }
            }
            "http.fetch" => {
                if running.probe || !running.activated.load(Ordering::Acquire) {
                    return Err(ProtocolError::new(
                        ErrorCode::PermissionDenied,
                        "Activation probes cannot access services",
                    ));
                }
                let context = self.context(running, &params).await?;
                let request: super::http::Request = serde_json::from_value(params.clone())
                    .map_err(|_| ProtocolError::invalid("Invalid HTTP request"))?;
                let grant = running
                    .manifest
                    .http
                    .iter()
                    .find(|g| g.origin == request.origin)
                    .ok_or_else(|| {
                        ProtocolError::new(
                            ErrorCode::NetworkDenied,
                            "Network origin was not granted",
                        )
                    })?;
                let credential = if let Some(id) = &grant.credential {
                    self.credentials
                        .get(
                            &running.installation_id,
                            &Credentials::key(id, &grant.origin),
                        )
                        .await?
                } else {
                    None
                };
                let response = running
                    .http
                    .fetch(
                        &running.manifest.id,
                        request,
                        grant,
                        credential,
                        &context.cancel,
                    )
                    .await?;
                self.contexts.lock().await.live(&context)?;
                Ok(serde_json::to_value(response).unwrap())
            }
            "ui.notify" => {
                if running.probe || !running.activated.load(Ordering::Acquire) {
                    return Err(ProtocolError::new(
                        ErrorCode::PermissionDenied,
                        "Activation probes cannot notify",
                    ));
                }
                let message = params["message"]
                    .as_str()
                    .filter(|s| s.len() <= 500)
                    .ok_or_else(|| ProtocolError::invalid("Invalid notification"))?;
                if !running
                    .notifications
                    .lock()
                    .await
                    .accept(Instant::now(), 3, 3)
                {
                    return Err(ProtocolError::new(
                        ErrorCode::ResourceLimit,
                        "Notification limit",
                    ));
                }
                self.effect(running, operation, json!({"message":message}), None, None)
                    .await
            }
            "panels.open" | "composerViews.open" | "composer.appendText" | "links.open" => {
                if running.probe || !running.activated.load(Ordering::Acquire) {
                    return Err(ProtocolError::new(
                        ErrorCode::PermissionDenied,
                        "Activation probes cannot affect the app",
                    ));
                }
                let context = self.context(running, &params).await?;
                if operation == "composer.appendText" {
                    super::permissions::require(&running.manifest, Permission::ComposerAppend)?;
                    if params["text"].as_str().is_none_or(|s| s.len() > 32768) {
                        return Err(ProtocolError::invalid("Insertion exceeds 32 KiB"));
                    }
                }
                if operation == "links.open" {
                    super::permissions::require(&running.manifest, Permission::ExternalOpen)?;
                    if !params["url"]
                        .as_str()
                        .is_some_and(codemux_addon_protocol::manifest::https_url)
                    {
                        return Err(ProtocolError::new(
                            ErrorCode::NetworkDenied,
                            "Only HTTPS links can be opened",
                        ));
                    }
                }
                if operation == "panels.open" || operation == "composerViews.open" {
                    let views = if operation == "panels.open" {
                        &running.manifest.contributes.panels
                    } else {
                        &running.manifest.contributes.composer_views
                    };
                    if !params["id"]
                        .as_str()
                        .is_some_and(|id| views.iter().any(|v| v.id == id))
                    {
                        return Err(ProtocolError::new(
                            ErrorCode::PermissionDenied,
                            "Contribution does not belong to this plugin",
                        ));
                    }
                }
                // Every app effect, including opening a panel, needs a fresh
                // user interaction, so a plugin cannot act on its own.
                self.contexts.lock().await.consume(
                    params["context"].as_str().unwrap(),
                    running.generation(),
                    Instant::now(),
                )?;
                if operation.starts_with("composer") && context.composer.is_none() {
                    // Panels bind a composer only when exactly one is open.
                    let several = match &context.workspace {
                        Some(w) => self.contexts.lock().await.composers_in(&w.id) > 1,
                        None => false,
                    };
                    return Err(ProtocolError::new(
                        ErrorCode::NoComposer,
                        if several {
                            "Several chat composers are open. Use the Add-ons menu in the composer you want."
                        } else {
                            "No chat composer is available"
                        },
                    ));
                }
                if context.workspace.is_none() {
                    return Err(ProtocolError::new(
                        ErrorCode::NoWorkspace,
                        "No workspace is open",
                    ));
                }
                let mut effect = params;
                effect["workspaceId"] = json!(context.workspace.as_ref().map(|w| &w.id));
                effect["composerId"] = json!(context.composer);
                effect["contextRevision"] = json!(context.revision);
                self.effect(
                    running,
                    operation,
                    effect,
                    Some(context.cancel.clone()),
                    context.interaction_deadline(),
                )
                .await
            }
            _ => Err(ProtocolError::invalid("Unknown SDK operation")),
        }
    }
    async fn effect(
        &self,
        running: &Running,
        operation: &str,
        params: Value,
        cancel: Option<CancellationToken>,
        deadline: Option<Instant>,
    ) -> Result<Value> {
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        let cancel = cancel.unwrap_or_else(|| running.cancel.clone());
        let deadline = deadline.unwrap_or_else(|| Instant::now() + Duration::from_secs(15));
        self.effects.lock().await.insert(
            id.clone(),
            PendingEffect {
                generation: running.generation().into(),
                sender: tx,
                cancel: cancel.clone(),
                deadline,
                claimed: None,
            },
        );
        let _ = self.events.send(UiEvent::Effect {
            plugin_id: running.manifest.id.clone(),
            generation: running.generation().into(),
            request_id: id.clone(),
            operation: operation.into(),
            params,
        });
        let mut rx = rx;
        let mut expires = deadline;
        let result = loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => break Err(ProtocolError::new(ErrorCode::ContextStale, "Target closed")),
                result = &mut rx => break result.map_err(|_| ProtocolError::new(ErrorCode::PluginStopped, "Plugin stopped")).and_then(|r| r),
                _ = tokio::time::sleep_until(expires.into()) => {
                    // A claim just before the deadline extends only the wait
                    // for its result, never the window for claiming it.
                    match self.effects.lock().await.get(&id).map(PendingEffect::expires) {
                        Some(extended) if extended > Instant::now() => expires = extended,
                        _ => break rx.try_recv().unwrap_or_else(|_| Err(ProtocolError::new(ErrorCode::Timeout, "Host UI did not respond"))),
                    }
                }
            }
        };
        self.effects.lock().await.remove(&id);
        result
    }
    pub async fn claim_effect(&self, id: &str, generation: &str) -> Result<()> {
        let mut effects = self.effects.lock().await;
        let effect = effects
            .get_mut(id)
            .filter(|e| {
                e.generation == generation
                    && !e.cancel.is_cancelled()
                    && e.claimed.is_none()
                    && Instant::now() < e.deadline
            })
            .ok_or_else(|| {
                ProtocolError::new(ErrorCode::ContextStale, "The add-on action expired")
            })?;
        effect.claimed = Some(Instant::now());
        Ok(())
    }
    pub async fn effect_result(
        &self,
        id: &str,
        generation: &str,
        result: Result<Value>,
    ) -> Result<()> {
        let mut effects = self.effects.lock().await;
        if effects.get(id).is_none_or(|effect| {
            effect.generation != generation
                || effect.cancel.is_cancelled()
                || Instant::now() >= effect.expires()
        }) {
            return Err(ProtocolError::new(
                ErrorCode::ContextStale,
                "Effect expired",
            ));
        }
        let effect = effects.remove(id).unwrap();
        let _ = effect.sender.send(result);
        Ok(())
    }
    pub async fn context_handle(
        &self,
        running: &Running,
        workspace: Option<Workspace>,
        composer: Option<String>,
    ) -> Result<String> {
        let mut contexts = self.contexts.lock().await;
        if let Some(id) = &composer {
            contexts.validate_composer(id, workspace.as_ref().map(|w| w.id.as_str()))?;
        }
        contexts.issue(running.generation(), workspace, composer)
    }
    /// Takes ownership of a handle issued for this execution: it becomes the
    /// command's interaction, or is revoked when the command cannot run.
    pub async fn execute(
        &self,
        running: &Running,
        id: &str,
        kind: &str,
        context: &str,
    ) -> Result<()> {
        let result = self.send_command(running, id, kind, context).await;
        if result.is_err() {
            self.contexts.lock().await.revoke(context);
        }
        result
    }
    async fn send_command(
        &self,
        running: &Running,
        id: &str,
        kind: &str,
        context: &str,
    ) -> Result<()> {
        let declared = if kind == "composerActions" {
            running
                .manifest
                .contributes
                .composer_actions
                .iter()
                .any(|c| c.id == id)
        } else if kind == "commands" {
            running
                .manifest
                .contributes
                .commands
                .iter()
                .any(|c| c.id == id)
        } else {
            false
        };
        if !declared {
            return Err(ProtocolError::invalid("Unknown command"));
        }
        let requires_workspace = kind == "composerActions"
            || running
                .manifest
                .contributes
                .commands
                .iter()
                .any(|c| c.id == id && c.requires_workspace);
        if requires_workspace
            && self
                .contexts
                .lock()
                .await
                .get(context, running.generation())?
                .workspace
                .is_none()
        {
            return Err(ProtocolError::new(
                ErrorCode::NoWorkspace,
                "This command requires a local workspace",
            ));
        }
        self.contexts
            .lock()
            .await
            .promote(context, running.generation(), Instant::now())?;
        *running.last_used.lock().await = Instant::now();
        running
            .host
            .send(
                "command.execute",
                json!({"id":id,"kind":kind,"context":context}),
            )
            .await?;
        Ok(())
    }
    /// Takes ownership of a handle issued for this view; a failed mount
    /// revokes it instead of leaving it until the next project switch.
    pub async fn mount(
        &self,
        running: &Running,
        id: &str,
        kind: &str,
        context: &str,
    ) -> Result<String> {
        let result = self.mount_view(running, id, kind, context).await;
        if result.is_err() {
            self.contexts.lock().await.revoke(context);
        }
        result
    }
    async fn mount_view(
        &self,
        running: &Running,
        id: &str,
        kind: &str,
        context: &str,
    ) -> Result<String> {
        let declared = if kind == "composerViews" {
            running
                .manifest
                .contributes
                .composer_views
                .iter()
                .any(|v| v.id == id)
        } else if kind == "panels" {
            running
                .manifest
                .contributes
                .panels
                .iter()
                .any(|v| v.id == id)
        } else {
            false
        };
        if !declared {
            return Err(ProtocolError::invalid("Unknown view"));
        }
        self.contexts
            .lock()
            .await
            .get(context, running.generation())?;
        let mut views = running.views.lock().await;
        if views.len() >= limits::VIEWS {
            return Err(ProtocolError::new(ErrorCode::ResourceLimit, "View limit"));
        }
        let view_id = format!(
            "view-{}",
            running.issued_views.fetch_add(1, Ordering::AcqRel) + 1
        );
        views.insert(
            view_id.clone(),
            View {
                tree: Tree::default(),
                context: context.into(),
                revision: 0,
                acknowledged: 0,
            },
        );
        drop(views);
        if let Err(error) = running
            .host
            .send(
                "view.mount",
                json!({"id":id,"kind":kind,"viewId":view_id,"context":context}),
            )
            .await
        {
            running.views.lock().await.remove(&view_id);
            return Err(error);
        }
        Ok(view_id)
    }
    pub async fn acknowledge(&self, running: &Running, view_id: &str, revision: u64) -> Result<()> {
        let mut views = running.views.lock().await;
        let view = views
            .get_mut(view_id)
            .ok_or_else(|| ProtocolError::new(ErrorCode::ContextStale, "View closed"))?;
        if revision > view.revision {
            return Err(ProtocolError::invalid("Invalid UI acknowledgement"));
        }
        if revision <= view.acknowledged {
            return Ok(());
        }
        view.acknowledged = revision;
        drop(views);
        running
            .host
            .send("ui.ack", json!({"viewId":view_id,"revision":revision}))
            .await
            .map(|_| ())
    }
    pub async fn ui_link(
        &self,
        running: &Running,
        view_id: &str,
        node_id: &str,
        url: &str,
    ) -> Result<()> {
        fn markdown(nodes: &[codemux_addon_protocol::ui::Node], id: &str) -> bool {
            nodes.iter().any(|n| {
                (n.id == id && n.element.as_deref() == Some("cmx-markdown"))
                    || markdown(&n.children, id)
            })
        }
        let views = running.views.lock().await;
        let view = views
            .get(view_id)
            .filter(|v| markdown(&v.tree.children, node_id))
            .ok_or_else(|| ProtocolError::new(ErrorCode::ContextStale, "Markdown link expired"))?;
        let context = self.contexts.lock().await.interact(
            &view.context,
            running.generation(),
            Instant::now(),
        )?;
        drop(views);
        // This URL comes only from a trusted renderer click, never child RPC.
        self.request(running, "links.open", json!({"context":context,"url":url}))
            .await?;
        Ok(())
    }
    pub async fn ui_event(
        &self,
        running: &Running,
        view_id: &str,
        node_id: &str,
        event: &str,
        callback_id: &str,
        value: Value,
    ) -> Result<()> {
        if !value.is_null()
            && !value.is_boolean()
            && !value.as_str().is_some_and(|s| s.len() <= 32768)
        {
            return Err(ProtocolError::invalid("Invalid UI event"));
        }
        let views = running.views.lock().await;
        let view = views
            .get(view_id)
            .ok_or_else(|| ProtocolError::new(ErrorCode::ContextStale, "View closed"))?;
        if !view.tree.callback(node_id, event, callback_id) {
            return Err(ProtocolError::new(
                ErrorCode::ContextStale,
                "UI callback expired",
            ));
        }
        let context = self.contexts.lock().await.interact(
            &view.context,
            running.generation(),
            Instant::now(),
        )?;
        drop(views);
        *running.last_used.lock().await = Instant::now();
        running
            .host
            .send(
                "ui.event",
                json!({"viewId":view_id,"callbackId":callback_id,"context":context,"value":value}),
            )
            .await?;
        Ok(())
    }
    pub async fn running(&self, id: &str, generation: &str) -> Result<Arc<Running>> {
        self.running
            .lock()
            .await
            .get(id)
            .filter(|r| r.generation() == generation && !r.cancel.is_cancelled())
            .cloned()
            .ok_or_else(|| {
                ProtocolError::new(ErrorCode::PluginStopped, "Plugin generation stopped")
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn test_host_path() -> PathBuf {
        std::env::var_os("CODEMUX_TEST_ADDON_HOST")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(if cfg!(windows) {
                    "addon-host/target/debug/codemux-addon-host.exe"
                } else {
                    "addon-host/target/debug/codemux-addon-host"
                })
            })
    }
    fn installed(manifest: Manifest) -> Installation {
        let source = Source::Local {
            identity: uuid::Uuid::new_v4().to_string(),
        };
        let installation_id = uuid::Uuid::new_v4().to_string();
        let digest = "a".repeat(64);
        let grant = Grant {
            installation_id: installation_id.clone(),
            plugin_id: manifest.id.clone(),
            source: source.clone(),
            digest: digest.clone(),
            capabilities: super::super::permissions::capability_digest(&manifest),
        };
        Installation {
            installation_id,
            manifest,
            source,
            digest,
            desired_enabled: true,
            status: Status::EnabledIdle,
            data_generation: uuid::Uuid::new_v4().to_string(),
            grant: Some(grant),
            failure: None,
            previous: None,
        }
    }
    #[tokio::test]
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_broker_scopes_storage_and_cancels_disposed_contexts() {
        let root = tempfile::tempdir().unwrap();
        let executable = test_host_path();
        assert!(executable.is_file(), "Build the standalone host first");
        let manager = Manager::open(root.path().into(), executable).unwrap();
        let manifest = Manifest::parse(
            include_bytes!("../../addon-protocol/fixtures/hello.json"),
            None,
        )
        .unwrap();
        let install = installed(manifest);
        manager.save(&install).unwrap();
        let source="__codemuxRegister({}, ({send}) => m => {if(m.method==='activate')send('ready',{phase:'activated',registrations:['commands/hello']});});";
        let running = manager
            .activate(install, source.into(), false)
            .await
            .unwrap();
        assert!(manager
            .request(&running, "invoke", json!({"command":"run_shell"}))
            .await
            .is_err());
        assert!(manager
            .request(&running, "workspace.current", json!({"context":"forged"}))
            .await
            .is_err());
        manager
            .request(
                &running,
                "storage.set",
                json!({"scope":"global","key":"test","value":{"saved":true}}),
            )
            .await
            .unwrap();
        assert_eq!(
            manager
                .request(
                    &running,
                    "storage.get",
                    json!({"scope":"global","key":"test"})
                )
                .await
                .unwrap(),
            json!({"saved":true})
        );
        assert!(manager
            .request(
                &running,
                "storage.get",
                json!({"scope":"global","key":"test","pluginId":"other"})
            )
            .await
            .is_err());
        let context = manager.context_handle(&running, None, None).await.unwrap();
        let cancel = manager
            .contexts
            .lock()
            .await
            .get(&context, running.generation())
            .unwrap()
            .cancel
            .clone();
        manager.change_workspace(None).await;
        assert!(cancel.is_cancelled());
        assert!(manager
            .contexts
            .lock()
            .await
            .get(&context, running.generation())
            .is_err());
        manager.stop(&running.manifest.id, None).await;
        assert!(manager
            .running(&running.manifest.id, running.generation())
            .await
            .is_err());
    }
    #[tokio::test]
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_uninstall_during_activation_reaps_the_candidate_before_removing_state() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().join("private"), test_host_path()).unwrap();
        // This fixture yields activation without registering ready. The test can
        // release it deterministically over its owned host's protocol; no public
        // application test bypass or timing-sensitive JS sleep is introduced.
        let source = b"__codemuxRegister({}, ({send}) => m => {if(m.method==='command.execute')send('ready',{phase:'activated',registrations:['commands/hello']});});";
        let package = root.path().join("activation.cmxaddon");
        std::fs::write(
            &package,
            super::super::package::fixture_archive_with_source(source),
        )
        .unwrap();
        let reviews = super::super::lifecycle::Reviews::default();
        let review = reviews.prepare_local(&manager, &package).unwrap();
        let mut installation = reviews
            .accept(&manager, &review.token, false, false)
            .await
            .unwrap();
        installation.desired_enabled = true;
        manager.save(&installation).unwrap();
        {
            let activation = manager.ensure_active(&installation.manifest.id);
            tokio::pin!(activation);
            let running = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                tokio::select! {
                    result = &mut activation => panic!("activation completed before release: {}", result.is_ok()),
                    _ = tokio::time::sleep(Duration::from_millis(5)) => {}
                }
                if let Some(running) = manager.running.lock().await.get(&installation.manifest.id).cloned() {
                    break running;
                }
            }
        }).await.expect("native activation starts");
            assert!(!running.activated.load(Ordering::Acquire));
            let removal = manager.remove(&installation.manifest.id, false);
            tokio::pin!(removal);
            assert!(
                tokio::time::timeout(Duration::from_millis(30), &mut removal)
                    .await
                    .is_err()
            );
            running
                .host
                .send("command.execute", json!({"id":"hello"}))
                .await
                .unwrap();
            let (activated, removed) = tokio::time::timeout(Duration::from_secs(2), async {
                tokio::join!(&mut activation, &mut removal)
            })
            .await
            .expect("activation and removal serialize");
            activated.unwrap();
            let warnings = removed.unwrap();
            assert!(warnings.is_empty(), "Removal cleanup: {warnings:?}");
            assert!(running.stopped.is_cancelled());
            // Keep this stale reference alive while checking cleanup. Windows
            // must not depend on Arc drop to release private database handles.
            assert!(running.storage.lock().unwrap().is_none());
            assert!(manager.running.lock().await.is_empty());
            assert!(manager.list().unwrap().is_empty());
            assert!(!manager
                .root
                .join("state")
                .join(&installation.installation_id)
                .exists());
            assert!(manager
                .ensure_active(&installation.manifest.id)
                .await
                .is_err());
        }
        drop(manager);
        assert!(Manager::open(root.path().join("private"), test_host_path())
            .unwrap()
            .list()
            .unwrap()
            .is_empty());
    }
    #[tokio::test]
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_delayed_effects_cannot_outlive_their_target_or_installation() {
        for action in [
            "workspace",
            "close",
            "replace",
            "disable",
            "remove",
            "pause",
        ] {
            let root = tempfile::tempdir().unwrap();
            let manager = Manager::open(root.path().into(), test_host_path()).unwrap();
            let mut manifest = Manifest::parse(
                include_bytes!("../../addon-protocol/fixtures/hello.json"),
                None,
            )
            .unwrap();
            manifest.permissions.push(Permission::ComposerAppend);
            let installation = installed(manifest);
            manager.save(&installation).unwrap();
            let source = "__codemuxRegister({}, ({send}) => m => {if(m.method==='activate')send('ready',{phase:'activated',registrations:['commands/hello']});});";
            let running = manager
                .activate(installation, source.into(), false)
                .await
                .unwrap();
            let composer = uuid::Uuid::new_v4().to_string();
            manager
                .contexts
                .lock()
                .await
                .register_composer(composer.clone(), "original".into())
                .unwrap();
            let workspace = Workspace {
                id: "original".into(),
                name: "Original project".into(),
                root_name: "project".into(),
                location: "local",
                root: root.path().into(),
            };
            let base = manager
                .context_handle(&running, Some(workspace), Some(composer.clone()))
                .await
                .unwrap();
            let interaction = manager
                .contexts
                .lock()
                .await
                .interact(&base, running.generation(), Instant::now())
                .unwrap();
            let mut events = manager.events.subscribe();
            let broker = manager.clone();
            let instance = running.clone();
            let request = tokio::spawn(async move {
                broker.request(&instance, "composer.appendText", json!({"context":interaction,"text":"Must never reach a replacement draft"})).await
            });
            let request_id = tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if let UiEvent::Effect {
                        request_id, params, ..
                    } = events.recv().await.unwrap()
                    {
                        assert_eq!(params["workspaceId"], "original");
                        assert_eq!(params["composerId"], composer);
                        break request_id;
                    }
                }
            })
            .await
            .unwrap();
            // Hold the frontend completion at the real native broker boundary.
            // Neither the claim nor a late result may survive a target change.
            assert!(manager
                .claim_effect(&request_id, "forged-generation")
                .await
                .is_err());
            match action {
                "workspace" => manager.change_workspace(None).await,
                "close" => manager.contexts.lock().await.revoke_composer(&composer),
                "replace" => manager
                    .contexts
                    .lock()
                    .await
                    .register_composer(composer.clone(), "replacement".into())
                    .unwrap(),
                "disable" => {
                    let mut installed = manager.installation(&running.manifest.id).unwrap();
                    installed.desired_enabled = false;
                    manager.save(&installed).unwrap();
                    manager.stop(&running.manifest.id, None).await;
                }
                "remove" => {
                    manager.remove(&running.manifest.id, false).await.unwrap();
                }
                "pause" => manager.pause_all().await,
                _ => unreachable!(),
            }
            assert!(
                manager
                    .claim_effect(&request_id, running.generation())
                    .await
                    .is_err(),
                "late claim after {action}"
            );
            assert!(
                manager
                    .effect_result(&request_id, running.generation(), Ok(json!(1)))
                    .await
                    .is_err(),
                "late completion after {action}"
            );
            let result = tokio::time::timeout(Duration::from_secs(2), request)
                .await
                .unwrap()
                .unwrap();
            assert!(result.is_err(), "delayed request after {action}");
            assert!(manager.effects.lock().await.is_empty());
            assert!(manager
                .contexts
                .lock()
                .await
                .get(&base, running.generation())
                .is_err());
            manager.shutdown().await;
            assert!(manager.running.lock().await.is_empty());
        }
    }
    #[tokio::test]
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_runtime_faults_quarantine_one_plugin_and_preserve_another() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().into(), test_host_path()).unwrap();
        let manifest = Manifest::parse(
            include_bytes!("../../addon-protocol/fixtures/hello.json"),
            None,
        )
        .unwrap();
        let mut healthy_manifest = manifest.clone();
        healthy_manifest.id = "example.healthy".into();
        let healthy_install = installed(healthy_manifest);
        manager.save(&healthy_install).unwrap();
        let healthy_source = "__codemuxRegister({}, ({send}) => m => {if(m.method==='activate')send('ready',{phase:'activated',registrations:['commands/hello']});});";
        let healthy = manager
            .activate(healthy_install, healthy_source.into(), false)
            .await
            .unwrap();
        let failing_installation = installed(manifest.clone());
        for code in [
            "throw Error('private failure details')",
            "while(true){}",
            "Promise.resolve().then(function loop(){Promise.resolve().then(loop)})",
            "function f(){f()} f()",
            "new ArrayBuffer(128*1024*1024)",
            // Ask only the child to exit: the manager has no pending stop and
            // must treat the resulting EOF as an unexpected runtime failure.
            "",
        ] {
            let installation = failing_installation.clone();
            manager.save(&installation).unwrap();
            let source = format!("__codemuxRegister({{}}, ({{send}}) => m => {{if(m.method==='activate')send('ready',{{phase:'activated',registrations:['commands/hello']}});else if(m.method==='command.execute'){{{code}}}}});");
            let failing = manager.activate(installation, source, false).await.unwrap();
            let context = manager.context_handle(&failing, None, None).await.unwrap();
            let start = Instant::now();
            failing
                .host
                .send(
                    if code.is_empty() {
                        "deactivate"
                    } else {
                        "command.execute"
                    },
                    json!({"id":"hello"}),
                )
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(2), failing.stopped.cancelled())
                .await
                .expect("fault contained and child reaped within two seconds");
            assert!(start.elapsed() < Duration::from_secs(2));
            assert!(matches!(
                manager.installation(&manifest.id).unwrap().status,
                Status::FailedDisabled
            ));
            assert!(manager
                .contexts
                .lock()
                .await
                .get(&context, failing.generation())
                .is_err());
            assert!(
                manager.ensure_active(&manifest.id).await.is_err(),
                "a fault cannot auto-restart the plugin"
            );
            assert!(!healthy.cancel.is_cancelled());
            assert_eq!(manager.running.lock().await.len(), 1);
            manager
                .request(
                    &healthy,
                    "storage.set",
                    json!({"scope":"global","key":"alive","value":true}),
                )
                .await
                .unwrap();
            assert_eq!(
                manager
                    .request(
                        &healthy,
                        "storage.get",
                        json!({"scope":"global","key":"alive"})
                    )
                    .await
                    .unwrap(),
                json!(true)
            );
        }
        manager.shutdown().await;
        assert!(manager.running.lock().await.is_empty());
    }
    #[tokio::test]
    #[ignore = "Build the independent Project Brief package and host first"]
    async fn native_project_brief_package_uses_installer_git_ui_and_composer_broker() {
        let root = tempfile::tempdir().unwrap();
        let executable = test_host_path();
        let manager = Manager::open(root.path().join("addons"), executable).unwrap();
        let package = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/addons/project-brief/codemux.project-brief-1.0.0.cmxaddon");
        let reviews = super::super::lifecycle::Reviews::default();
        let review = reviews.prepare_local(&manager, &package).unwrap();
        assert!(
            manager.running.lock().await.is_empty(),
            "Review must not execute code"
        );
        let installed = reviews
            .accept(&manager, &review.token, true, false)
            .await
            .unwrap();
        assert!(
            manager.running.lock().await.values().all(|r| !r.probe),
            "Only the normally activated generation may remain"
        );
        let git_root = root.path().join("project");
        std::fs::create_dir(&git_root).unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(&git_root)
            .status()
            .unwrap()
            .success());
        std::fs::write(git_root.join("example.txt"), "synthetic fixture").unwrap();
        let workspace = Workspace {
            id: "test-workspace".into(),
            name: "Native test project".into(),
            root_name: "project".into(),
            location: "local",
            root: git_root,
        };
        let composer = uuid::Uuid::new_v4().to_string();
        manager
            .contexts
            .lock()
            .await
            .register_composer(composer.clone(), workspace.id.clone())
            .unwrap();
        let mut events = manager.events.subscribe();
        let running = manager.ensure_active(&installed.manifest.id).await.unwrap();
        let context = manager
            .context_handle(&running, Some(workspace), Some(composer.clone()))
            .await
            .unwrap();
        let view = manager
            .mount(&running, "brief", "panels", &context)
            .await
            .unwrap();
        fn text(node: &codemux_addon_protocol::ui::Node) -> String {
            node.data.clone().unwrap_or_default()
                + &node.children.iter().map(text).collect::<String>()
        }
        fn button(nodes: &[codemux_addon_protocol::ui::Node]) -> Option<(String, String)> {
            for node in nodes {
                if node.element.as_deref() == Some("cmx-button")
                    && text(node) == "Add to draft"
                    && node.properties.get("disabled") != Some(&json!(true))
                {
                    return Some((
                        node.id.clone(),
                        node.event_listeners["press"]["callbackId"]
                            .as_str()
                            .unwrap()
                            .into(),
                    ));
                }
                if let Some(found) = button(&node.children) {
                    return Some(found);
                }
            }
            None
        }
        let (node, callback) = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match events.recv().await.unwrap() {
                    UiEvent::Tree { tree, revision, .. } => {
                        manager
                            .acknowledge(&running, &view, revision)
                            .await
                            .unwrap();
                        if let Some(found) = button(&tree.children) {
                            break found;
                        }
                    }
                    UiEvent::Stopped { message, .. } => panic!("Plugin stopped: {message}"),
                    _ => {}
                }
            }
        })
        .await
        .expect("Project Brief should render real Git data");
        manager
            .ui_event(&running, &view, &node, "press", &callback, Value::Null)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match events.recv().await.unwrap() {
                    UiEvent::Tree { revision, .. } => manager
                        .acknowledge(&running, &view, revision)
                        .await
                        .unwrap(),
                    UiEvent::Effect {
                        request_id,
                        generation,
                        operation,
                        params,
                        ..
                    } => {
                        assert_eq!(operation, "composer.appendText");
                        assert_eq!(params["composerId"], composer);
                        assert!(params["text"]
                            .as_str()
                            .unwrap()
                            .contains("Native test project"));
                        assert!(params["text"].as_str().unwrap().contains("example.txt"));
                        manager
                            .claim_effect(&request_id, &generation)
                            .await
                            .unwrap();
                        manager
                            .effect_result(&request_id, &generation, Ok(json!(1)))
                            .await
                            .unwrap();
                        break;
                    }
                    UiEvent::Stopped { message, .. } => panic!("Plugin stopped: {message}"),
                    _ => {}
                }
            }
        })
        .await
        .expect("Real SDK button should reach the scoped composer broker");
        manager.remove(&installed.manifest.id, false).await.unwrap();
        assert!(manager.list().unwrap().is_empty());
        assert!(manager.running.lock().await.is_empty());
    }
    #[tokio::test]
    #[ignore = "Build independent Issue Companion and host first"]
    async fn native_issue_companion_uses_broker_transport_and_explicit_composer_callback() {
        let root = tempfile::tempdir().unwrap();
        let executable = test_host_path();
        let manager = Manager::open(root.path().join("addons"), executable).unwrap();
        let package = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../examples/addons/issue-companion/codemux.issue-companion-1.0.0.cmxaddon");
        let reviews = super::super::lifecycle::Reviews::default();
        let review = reviews.prepare_local(&manager, &package).unwrap();
        let installed = reviews
            .accept(&manager, &review.token, true, false)
            .await
            .unwrap();
        manager
            .set_settings(
                &installed.manifest.id,
                json!({"owner":"synthetic-owner","repository":"fixture-repository"}),
            )
            .await
            .unwrap();
        let workspace = Workspace {
            id: "issue-fixture".into(),
            name: "Synthetic project".into(),
            root_name: "project".into(),
            location: "local",
            root: root.path().into(),
        };
        let composer = uuid::Uuid::new_v4().to_string();
        manager
            .contexts
            .lock()
            .await
            .register_composer(composer.clone(), workspace.id.clone())
            .unwrap();
        let running = manager.ensure_active(&installed.manifest.id).await.unwrap();
        let mut events = manager.events.subscribe();
        fn content(node: &codemux_addon_protocol::ui::Node) -> String {
            node.data.clone().unwrap_or_default()
                + &node.children.iter().map(content).collect::<String>()
        }
        fn find_button(nodes: &[codemux_addon_protocol::ui::Node]) -> Option<(String, String)> {
            for node in nodes {
                if node.element.as_deref() == Some("cmx-button")
                    && content(node) == "Add to draft"
                    && node.properties.get("disabled") != Some(&json!(true))
                {
                    return Some((
                        node.id.clone(),
                        node.event_listeners["press"]["callbackId"]
                            .as_str()
                            .unwrap()
                            .into(),
                    ));
                }
                if let Some(found) = find_button(&node.children) {
                    return Some(found);
                }
            }
            None
        }
        let mut final_view = None;
        for (status,body,headers,expected) in [
            (401,"{}".to_owned(),std::collections::BTreeMap::new(),"did not accept"),
            (429,"{}".to_owned(),std::collections::BTreeMap::new(),"rate limit"),
            (200,"[]".to_owned(),std::collections::BTreeMap::new(),"No open issues"),
            (200,json!([{"number":7,"title":"Synthetic issue title","html_url":"https://github.com/synthetic-owner/fixture-repository/issues/7"}]).to_string(),std::collections::BTreeMap::new(),"Synthetic issue title"),
        ] {
            running.http.recorded_responses(vec![super::super::http::Response{status,headers,body}]).await;
            let context=manager.context_handle(&running,Some(workspace.clone()),Some(composer.clone())).await.unwrap();let view=manager.mount(&running,"issues","panels",&context).await.unwrap();
            let callback=tokio::time::timeout(Duration::from_secs(5),async { loop { match events.recv().await.unwrap() {
                UiEvent::Tree{tree,revision,view_id,..} if view_id==view => {manager.acknowledge(&running,&view,revision).await.unwrap();if tree.children.iter().map(content).collect::<String>().contains(expected){break find_button(&tree.children)}},
                UiEvent::Stopped{message,..}=>panic!("Issue Companion stopped: {message}"),
                UiEvent::Effect{..}=>panic!("Rendering may not insert text or open links"),
                _=>{}
            }}}).await.expect("Issue Companion should render recorded broker result");
            if let Some(callback)=callback { final_view=Some((view,callback)); } else { manager.unmount(&running,&view).await.unwrap(); }
        }
        let observed = running.http.observed_requests().await;
        assert_eq!(observed.len(), 4);
        for request in observed {
            assert_eq!(request["origin"], "https://api.github.com");
            assert_eq!(
                request["path"],
                "/repos/synthetic-owner/fixture-repository/issues?state=open&per_page=50"
            );
            assert_eq!(request["method"], "GET");
            assert_eq!(request["body"], Value::Null);
            assert_eq!(request["credentialAttached"], false);
        }
        let (view, (node, callback)) = final_view.expect("Loaded issue has explicit action");
        manager
            .ui_event(&running, &view, &node, "press", &callback, Value::Null)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5),async {loop{match events.recv().await.unwrap(){
            UiEvent::Effect{request_id,generation,operation,params,..}=>{assert_eq!(operation,"composer.appendText");assert_eq!(params["composerId"],composer);assert_eq!(params["text"],"Synthetic issue title\nhttps://github.com/synthetic-owner/fixture-repository/issues/7");manager.claim_effect(&request_id,&generation).await.unwrap();manager.effect_result(&request_id,&generation,Ok(json!(1))).await.unwrap();break},
            UiEvent::Tree{revision,..}=>manager.acknowledge(&running,&view,revision).await.unwrap(),
            UiEvent::Stopped{message,..}=>panic!("Issue Companion stopped: {message}"),_=>{}
        }}}).await.unwrap();
        manager.stop(&installed.manifest.id, None).await;
        assert!(manager.running.lock().await.is_empty());
    }
    #[tokio::test]
    async fn corrupt_registry_returns_an_error_without_touching_core_state() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("registry.sqlite"),
            b"not a sqlite database",
        )
        .unwrap();
        assert!(Manager::open(root.path().into(), PathBuf::from("unused")).is_err());
    }
}
