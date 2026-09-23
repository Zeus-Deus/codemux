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
use codemux_addon_protocol::{
    limits,
    manifest::{Permission, Platform},
    ui::Tree,
};
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
use tokio::sync::{broadcast, oneshot, Mutex, Notify, Semaphore};
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
/// The plugin API this app provides.
pub const HOST_API: semver::Version = semver::Version::new(1, 0, 0);
/// Compatibility explanation for the Settings detail view.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Compatibility {
    pub api: String,
    pub host_api: String,
    pub platforms: Vec<Platform>,
    pub platform: Option<Platform>,
    pub compatible: bool,
    pub reason: Option<String>,
}
/// Structural manifest validation that tolerates another API range. A stored
/// package for an unsupported API is a valid, inert record, not corruption.
fn structural(manifest: &Manifest) -> Result<()> {
    match manifest.validate(None) {
        Err(error) if error.data.code == ErrorCode::IncompatibleApi => {
            let mut supported = manifest.clone();
            supported.api = format!("^{HOST_API}");
            supported.validate(None)
        }
        result => result,
    }
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
        structural(&self.manifest)?;
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
            structural(&previous.manifest)?;
        }
        Ok(())
    }
    pub fn compatibility(&self) -> Compatibility {
        let name = |p: &Platform| {
            serde_json::to_value(p)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        };
        let platform = super::catalog::platform().ok();
        let reason = if !semver::VersionReq::parse(&self.manifest.api)
            .is_ok_and(|range| range.matches(&HOST_API))
        {
            Some(format!(
                "Requires plugin API {}; this CodeMux supports {HOST_API}",
                self.manifest.api
            ))
        } else if let Some(platform) = platform {
            (!self.manifest.platforms.contains(&platform)).then(|| {
                format!(
                    "Built for {}; this device is {}",
                    self.manifest
                        .platforms
                        .iter()
                        .map(name)
                        .collect::<Vec<_>>()
                        .join(", "),
                    name(&platform)
                )
            })
        } else {
            Some("Add-ons are unsupported on this platform".into())
        };
        Compatibility {
            api: self.manifest.api.clone(),
            host_api: HOST_API.to_string(),
            platforms: self.manifest.platforms.clone(),
            platform,
            compatible: reason.is_none(),
            reason,
        }
    }
    /// Derive incompatible-disabled from the running app instead of storing
    /// it, so an app upgrade or downgrade re-evaluates every record.
    fn with_compatibility(mut self) -> Self {
        if matches!(self.status, Status::BlockedDisabled | Status::Removing) {
            return self;
        }
        match self.compatibility().reason {
            Some(reason) => {
                self.status = Status::IncompatibleDisabled;
                self.failure = Some(reason);
            }
            None if matches!(self.status, Status::IncompatibleDisabled) => {
                self.status = if self.desired_enabled {
                    Status::EnabledIdle
                } else {
                    Status::InstalledDisabled
                };
                self.failure = None;
            }
            None => {}
        }
        self
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
    ui_rate: Mutex<(limits::RateLimit, VecDeque<Instant>)>,
    requests: Arc<Semaphore>,
    git: Git,
    http: Http,
    last_used: StdMutex<Instant>,
    activity: Notify,
    notifications: Mutex<limits::RateLimit>,
}
impl Running {
    pub fn generation(&self) -> &str {
        &self.host.generation
    }
    /// Records use and wakes the idle supervisor to re-evaluate.
    fn touch(&self) {
        *self.last_used.lock().unwrap() = Instant::now();
        self.activity.notify_one();
    }
    /// When the host became idle: activated, no mounted UI, and no pending call
    /// in either direction. It counts from the later of the last recorded use
    /// and the last yielded call. Both the idle stop and eviction use it.
    fn idle_since(&self) -> Option<Instant> {
        let idle = self.activated.load(Ordering::Acquire)
            && !self.probe
            && !self.cancel.is_cancelled()
            && self.requests.available_permits() == 16
            && self.views.try_lock().is_ok_and(|views| views.is_empty());
        let settled = self.host.settled().filter(|_| idle)?;
        Some(settled.max(*self.last_used.lock().unwrap()))
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
/// Sanitized per-installation log activity. It outlives the generation that
/// produced it, so a stopped or crashed plugin keeps its diagnostics.
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostics {
    /// Entries received this session, including ones the ring evicted.
    pub received: u64,
    pub logs: VecDeque<LogEntry>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    /// Milliseconds since the Unix epoch.
    pub at: u64,
    pub level: &'static str,
    pub bytes: usize,
}
/// Each serialized entry is at most 64 bytes, bounding the ring to 64 KiB.
const LOG_ENTRIES: usize = 65536 / 64;
impl Diagnostics {
    fn record(&mut self, entry: LogEntry) {
        self.received += 1;
        self.logs.push_back(entry);
        while self.logs.len() > LOG_ENTRIES {
            self.logs.pop_front();
        }
    }
}
/// The trusted host paces UI batches to 30/s and 1,800/min at the source. This
/// backstop bounds only a broken host: it allows twice that to absorb arrival
/// bunching, never drops a batch, and stops only on repeated excess.
fn ui_traffic(rate: &mut (limits::RateLimit, VecDeque<Instant>), now: Instant) -> Result<()> {
    if !rate.0.accept(now, 60, 3600) && super::protocol::repeated(&mut rate.1, now) {
        return Err(ProtocolError::new(
            ErrorCode::ResourceLimit,
            "UI traffic limit",
        ));
    }
    Ok(())
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
    pub(super) catalog: super::catalog::CatalogState,
    running: Arc<Mutex<HashMap<String, Arc<Running>>>>,
    operations: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    effects: Mutex<HashMap<String, PendingEffect>>,
    pub events: broadcast::Sender<UiEvent>,
    paused: AtomicBool,
    pub(super) diagnostics: StdMutex<HashMap<String, Diagnostics>>,
    timing: Timing,
}
/// Supervision delays. Tests shorten them; they grant no authority.
#[derive(Clone, Copy)]
struct Timing {
    /// Stop a host this long after its last call with no mounted UI.
    idle: Duration,
    request: Duration,
    fetch: Duration,
}
impl Default for Timing {
    fn default() -> Self {
        Self {
            idle: Duration::from_secs(60),
            request: Duration::from_secs(15),
            fetch: Duration::from_secs(30),
        }
    }
}
impl Timing {
    fn request(&self, operation: &str) -> Duration {
        if operation == "http.fetch" {
            self.fetch
        } else {
            self.request
        }
    }
}
fn storage_error(_: rusqlite::Error) -> ProtocolError {
    ProtocolError::new(
        ErrorCode::StorageUnavailable,
        "Add-on registry is unavailable; plugins are paused",
    )
}
impl Manager {
    pub fn open(root: PathBuf, host_path: PathBuf) -> Result<Arc<Self>> {
        Self::open_with(root, host_path, Timing::default())
    }
    fn open_with(root: PathBuf, host_path: PathBuf, timing: Timing) -> Result<Arc<Self>> {
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
                return Err(ProtocolError::new(
                    ErrorCode::StorageUnavailable,
                    "The add-on credential index is damaged",
                ));
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
            catalog: Default::default(),
            running: Arc::new(Mutex::new(HashMap::new())),
            operations: Mutex::new(HashMap::new()),
            effects: Mutex::new(HashMap::new()),
            events,
            paused: AtomicBool::new(
                stored_paused
                    || std::env::var_os("CODEMUX_DISABLE_ADDONS").is_some_and(|v| v == "1"),
            ),
            diagnostics: StdMutex::new(HashMap::new()),
            timing,
        });
        manager.recover()?;
        manager.finish_removals()?;
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
    /// Log activity of the current installation this session, across generations.
    pub fn diagnostics(&self, id: &str) -> Result<Diagnostics> {
        let installation = self.installation(id)?;
        Ok(self
            .diagnostics
            .lock()
            .unwrap()
            .get(&installation.installation_id)
            .cloned()
            .unwrap_or_default())
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
            list.push(record.with_compatibility());
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
            // Evict the least recently used idle host; never one that is
            // activating, rendering, or serving a call.
            let idle = hosts
                .iter()
                .filter_map(|(id, r)| Some((r.idle_since()?, id)))
                .min()
                .map(|(_, id)| id.clone());
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
        if !probe {
            let mut activating = installation.clone();
            activating.status = Status::Activating;
            self.save(&activating)?;
        }
        self.registry
            .lock()
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO metadata(key,value) VALUES(?1,'pending')",
                [format!("activation:{}", installation.manifest.id)],
            )
            .map_err(storage_error)?;
        let (host, mut events) =
            match Host::spawn(&self.host_path, &installation.manifest, &source).await {
                Ok(spawned) => spawned,
                Err(error) => {
                    // No child started: not an unclean exit, and not starting.
                    let _ = self.registry.lock().unwrap().execute(
                        "DELETE FROM metadata WHERE key=?1",
                        [format!("activation:{}", installation.manifest.id)],
                    );
                    if !probe {
                        let _ = self.save(&installation);
                    }
                    return Err(error);
                }
            };
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
            ui_rate: Mutex::new(Default::default()),
            requests: Arc::new(Semaphore::new(16)),
            git: Git::default(),
            http: Http::default(),
            last_used: StdMutex::new(Instant::now()),
            activity: Notify::new(),
            notifications: Mutex::new(limits::RateLimit::default()),
        });
        hosts.insert(installation.manifest.id.clone(), running.clone());
        drop(hosts);
        let (ready, mut readiness) = oneshot::channel();
        let manager = self.clone();
        let instance = running.clone();
        tokio::spawn(async move {
            let mut ready = Some(ready);
            loop {
                // Stop 60 s after the last completed call with no mounted UI.
                // A busy host re-checks after each activity and at least once
                // per idle period.
                let idle_at =
                    instance.idle_since().unwrap_or_else(Instant::now) + manager.timing.idle;
                tokio::select! {
                 _=instance.cancel.cancelled()=>break,
                 _=instance.activity.notified()=>{},
                 _=tokio::time::sleep_until(idle_at.into())=>{if instance.idle_since().is_some_and(|since|since.elapsed()>=manager.timing.idle){manager.stop(&instance.manifest.id,None).await;break}},
                 event=events.recv()=>{let result=match event{
                  Some(Event::Message(message))=>{
                   if message.method.as_deref()==Some("ready")&&message.params.as_ref().is_some_and(|p|p["phase"]=="activated"){
                    let declarations=&instance.manifest.contributes;let mut expected=Vec::new();for c in &declarations.commands{expected.push(format!("commands/{}",c.id))}for (kind,views) in [("panels",&declarations.panels),("composerActions",&declarations.composer_actions),("composerViews",&declarations.composer_views)]{for v in views{expected.push(format!("{kind}/{}",v.id))}}expected.sort();
                    let mut actual:Vec<String>=serde_json::from_value(message.params.unwrap()["registrations"].clone()).unwrap_or_default();actual.sort();
                    if actual!=expected{Err(ProtocolError::invalid("Plugin registration does not match its manifest"))}else{instance.activated.store(true,Ordering::Release);if let Some(ready)=ready.take(){let _=ready.send(Ok(()));}Ok(())}
                   }else{manager.message(&instance,message).await}
                  },Some(Event::Stopped(error))=>Err(error),None=>Err(ProtocolError::new(ErrorCode::PluginStopped,"Plugin host exited"))};
                  if let Err(error)=result{if let Some(ready)=ready.take(){let _=ready.send(Err(error.clone()));}manager.stop(&instance.manifest.id,Some(error.message)).await;break}
                 }
                }
            }
        });
        if let Err(error) = running.host.send("activate", json!({})).await {
            // A child that died at once may already be stopped by the supervisor
            // with its cause. Stop only a generation nobody is stopping: without
            // an instance, stop cannot tell a probe from the installed record.
            let error = match readiness.try_recv() {
                Ok(Err(cause)) => cause,
                _ => error,
            };
            if running.cancel.is_cancelled() {
                let _ =
                    tokio::time::timeout(Duration::from_secs(2), running.stopped.cancelled()).await;
            } else {
                self.stop(&installation.manifest.id, Some(error.message.clone()))
                    .await;
            }
            return Err(error);
        }
        match tokio::time::timeout(Duration::from_secs(2), readiness).await {
            Ok(Ok(Ok(()))) => {}
            // The supervisor already stops this generation with the specific
            // cause. Report it; a second stop would overwrite the stored reason
            // or, for a probe, touch the installed record.
            Ok(Ok(Err(error))) => {
                let _ =
                    tokio::time::timeout(Duration::from_secs(2), running.stopped.cancelled()).await;
                return Err(error);
            }
            // Another operation (pause, disable, removal) stopped it first. A
            // supervisor that ended without stopping it leaves it to this caller.
            Ok(Err(_)) => {
                if running.cancel.is_cancelled() {
                    let _ =
                        tokio::time::timeout(Duration::from_secs(2), running.stopped.cancelled())
                            .await;
                } else {
                    self.stop(&installation.manifest.id, None).await;
                }
                return Err(ProtocolError::new(
                    ErrorCode::PluginStopped,
                    "Add-on stopped during activation",
                ));
            }
            Err(_) => {
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
            // Including the notice that an earlier start was interrupted.
            installation.failure = None;
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
                let manager = self.clone();
                let instance = running.clone();
                // Like the rate quota, the outstanding bound answers instead of
                // stopping; that quota also bounds these rejections.
                let Ok(permit) = running.requests.clone().try_acquire_owned() else {
                    tokio::spawn(async move {
                        let error =
                            ProtocolError::new(ErrorCode::ResourceLimit, "Too many host requests");
                        let _ = instance.host.respond(id, Err(error)).await;
                    });
                    return Ok(());
                };
                tokio::spawn(async move {
                    let operation = params["operation"].as_str().unwrap_or("");
                    let timeout = manager.timing.request(operation);
                    let result = tokio::select! {_ = instance.cancel.cancelled()=>Err(ProtocolError::new(ErrorCode::PluginStopped,"Plugin stopped")),result=tokio::time::timeout(timeout,manager.request(&instance,operation,params["params"].clone()))=>result.unwrap_or_else(|_|Err(ProtocolError::new(ErrorCode::Timeout,"Host request timed out")))};
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
                    // The idle period starts when the last call completes.
                    drop(permit);
                    instance.touch();
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
                ui_traffic(&mut *running.ui_rate.lock().await, Instant::now())?;
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
                // only the time, level and size, never raw author output.
                let level = ["info", "warn", "error", "debug"]
                    .into_iter()
                    .find(|known| params["level"].as_str() == Some(*known))
                    .unwrap_or("log");
                let entry = LogEntry {
                    at: chrono::Utc::now().timestamp_millis().max(0) as u64,
                    level,
                    bytes: params["message"].as_str().map_or(0, str::len),
                };
                self.diagnostics
                    .lock()
                    .unwrap()
                    .entry(running.installation_id.clone())
                    .or_default()
                    .record(entry);
                Ok(())
            }
            Some("ready") => {
                // The transport forwards only yields that end a pending call.
                if params["requestId"].is_u64() {
                    running.touch();
                }
                Ok(())
            }
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
                Status::BlockedDisabled
                    | Status::IncompatibleDisabled
                    | Status::Removing
                    | Status::Updating
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
        running.touch();
        Ok(())
    }
    pub async fn shutdown(&self) {
        self.catalog.stop.cancel();
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
            .execute_batch(
                "INSERT OR REPLACE INTO metadata(key,value) VALUES('paused','false'); DELETE FROM metadata WHERE key='interrupted';",
            )
            .map_err(storage_error)?;
        self.paused.store(false, Ordering::Release);
        let _ = self.events.send(UiEvent::Inventory);
        Ok(())
    }
    /// Add-ons whose activation an unclean exit interrupted, until Resume.
    pub fn interrupted_activations(&self) -> Vec<String> {
        self.registry
            .lock()
            .unwrap()
            .query_row(
                "SELECT value FROM metadata WHERE key='interrupted'",
                [],
                |row| row.get::<_, String>(0),
            )
            .ok()
            .and_then(|value| serde_json::from_str(&value).ok())
            .unwrap_or_default()
    }
    fn recover_session(&self) -> Result<()> {
        let interrupted: Vec<String> = {
            let db = self.registry.lock().unwrap();
            let mut query = db
                .prepare("SELECT key FROM metadata WHERE key LIKE 'activation:%' LIMIT 1000")
                .map_err(storage_error)?;
            let keys = query
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(storage_error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(storage_error)?;
            keys.into_iter()
                .filter_map(|key| key.strip_prefix("activation:").map(str::to_owned))
                .collect()
        };
        let installed = self.list()?;
        if !interrupted.is_empty() {
            // A probe for an installation that recovery rolled back still
            // pauses, but only installed add-ons can be named.
            let named: Vec<&String> = interrupted
                .iter()
                .filter(|id| installed.iter().any(|i| &&i.manifest.id == id))
                .collect();
            let db = self.registry.lock().unwrap();
            db.execute_batch("INSERT OR REPLACE INTO metadata(key,value) VALUES('paused','true'); DELETE FROM metadata WHERE key LIKE 'activation:%';").map_err(storage_error)?;
            db.execute(
                "INSERT OR REPLACE INTO metadata(key,value) VALUES('interrupted',?1)",
                [serde_json::to_string(&named).unwrap()],
            )
            .map_err(storage_error)?;
            self.paused.store(true, Ordering::Release);
        }
        for mut installation in installed {
            let starting = interrupted.contains(&installation.manifest.id);
            let running = matches!(
                installation.status,
                Status::Activating | Status::EnabledRunning | Status::Updating
            );
            if running {
                installation.status = if installation.desired_enabled {
                    Status::EnabledIdle
                } else {
                    Status::InstalledDisabled
                };
            }
            // Attribute the unclean exit to the add-on that was starting only.
            if starting {
                installation.failure = Some(
                    "CodeMux closed while this add-on was starting. Add-ons are paused; review it before resuming.".into(),
                );
            }
            if running || starting {
                self.save(&installation)?;
            }
        }
        Ok(())
    }
    /// Current state for a subscriber that missed broadcast events: inventory
    /// plus the latest tree of every mounted view.
    pub async fn resync_events(&self) -> Vec<UiEvent> {
        let mut events = vec![UiEvent::Inventory];
        let hosts: Vec<_> = self.running.lock().await.values().cloned().collect();
        for running in hosts {
            for (view_id, view) in running.views.lock().await.iter() {
                events.push(UiEvent::Tree {
                    plugin_id: running.manifest.id.clone(),
                    generation: running.generation().into(),
                    view_id: view_id.clone(),
                    revision: view.revision,
                    tree: view.tree.clone(),
                });
            }
        }
        events
    }
    /// Move an unreadable add-on directory aside, whole, so a fresh registry
    /// can open without pruning its packages or private data. Returns the backup.
    pub fn reset_registry(root: &std::path::Path) -> Result<PathBuf> {
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "addons".into());
        let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
        let mut backup = root.with_file_name(format!("{name}-backup-{stamp}"));
        if backup.exists() {
            backup = root.with_file_name(format!("{name}-backup-{stamp}-{}", uuid::Uuid::new_v4()));
        }
        std::fs::rename(root, &backup).map_err(|_| {
            ProtocolError::new(
                ErrorCode::StorageUnavailable,
                format!(
                    "Could not move {} aside. Close other CodeMux windows and retry.",
                    root.display()
                ),
            )
        })?;
        Ok(backup)
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
        running.touch();
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
        running.touch();
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
    /// A plugin with a command and a panel, as a real host would register it.
    fn panel_plugin(id: &str) -> Installation {
        let mut manifest: Value =
            serde_json::from_slice(include_bytes!("../../addon-protocol/fixtures/hello.json"))
                .unwrap();
        manifest["id"] = json!(id);
        manifest["contributes"]["panels"] =
            json!([{"id":"view","title":"View","icon":"file-text"}]);
        installed(Manifest::parse(&serde_json::to_vec(&manifest).unwrap(), None).unwrap())
    }
    const PANEL_SOURCE: &str = "__codemuxRegister({}, ({send}) => m => {if(m.method==='activate')send('ready',{phase:'activated',registrations:['commands/hello','panels/view']});else if(m.method==='command.execute')setTimeout(()=>send('host.request',{operation:'settings.get',params:{}},1),300);});";
    async fn show(manager: &Manager, running: &Running) -> String {
        let context = manager.context_handle(running, None, None).await.unwrap();
        manager
            .mount(running, "view", "panels", &context)
            .await
            .unwrap()
    }
    #[tokio::test]
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_idle_stop_waits_for_views_and_the_last_completed_call() {
        let root = tempfile::tempdir().unwrap();
        let timing = Timing {
            idle: Duration::from_millis(400),
            ..Timing::default()
        };
        let manager = Manager::open_with(root.path().into(), test_host_path(), timing).unwrap();
        let installation = panel_plugin("example.idle");
        manager.save(&installation).unwrap();
        let running = manager
            .activate(installation, PANEL_SOURCE.into(), false)
            .await
            .unwrap();
        let view = show(&manager, &running).await;
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert!(!running.cancel.is_cancelled(), "a mounted view is not idle");
        manager.unmount(&running, &view).await.unwrap();
        // The command awaits a 300 ms timer and then calls the broker. The
        // idle period starts when that call completes, not at the command.
        let context = manager.context_handle(&running, None, None).await.unwrap();
        let started = Instant::now();
        manager
            .execute(&running, "hello", "commands", &context)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), running.stopped.cancelled())
            .await
            .expect("an idle host stops");
        let elapsed = started.elapsed();
        assert!(elapsed >= Duration::from_millis(650), "{elapsed:?}");
        let installed = manager.installation(&running.manifest.id).unwrap();
        assert!(matches!(installed.status, Status::EnabledIdle));
        assert!(installed.failure.is_none());
    }
    #[tokio::test]
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_host_serving_a_parent_call_is_neither_idle_nor_stopped() {
        let root = tempfile::tempdir().unwrap();
        let timing = Timing {
            idle: Duration::from_millis(300),
            ..Timing::default()
        };
        let manager = Manager::open_with(root.path().into(), test_host_path(), timing).unwrap();
        let installation = installed(
            Manifest::parse(
                include_bytes!("../../addon-protocol/fixtures/hello.json"),
                None,
            )
            .unwrap(),
        );
        manager.save(&installation).unwrap();
        // The command runs 200 ms of synchronous work inside its 250 ms budget.
        let source = "__codemuxRegister({}, ({send}) => m => {\
            if(m.method==='activate')send('ready',{phase:'activated',registrations:['commands/hello']});\
            else if(m.method==='command.execute'){const end=Date.now()+200;while(Date.now()<end){}console.info('done');}});";
        let running = manager
            .activate(installation, source.into(), false)
            .await
            .unwrap();
        let activated = Instant::now();
        tokio::time::sleep(Duration::from_millis(150)).await;
        // Sent directly, so no recorded use: only the pending call keeps the
        // host from idling when its 300 ms idle period ends mid-call.
        running
            .host
            .send("command.execute", json!({"id":"hello"}))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            running.idle_since().is_none(),
            "a host serving a call cannot be evicted"
        );
        tokio::time::timeout(Duration::from_secs(3), running.stopped.cancelled())
            .await
            .expect("an idle host stops");
        let elapsed = activated.elapsed();
        let installed = manager.installation(&running.manifest.id).unwrap();
        assert_eq!(installed.failure, None);
        assert!(matches!(installed.status, Status::EnabledIdle));
        // The idle period restarts when the call yields, about 350 ms in.
        assert!(elapsed >= Duration::from_millis(600), "{elapsed:?}");
        let diagnostics = manager.diagnostics(&running.manifest.id).unwrap();
        assert!(diagnostics
            .logs
            .iter()
            .any(|entry| entry.level == "info" && entry.bytes == "done".len()));
    }
    #[tokio::test]
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_ui_updates_faster_than_the_batch_limit_are_paced_not_quarantined() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().into(), test_host_path()).unwrap();
        let installation = panel_plugin("example.fast-ui");
        manager.save(&installation).unwrap();
        // Renders again as soon as each batch is acknowledged, with no pacing.
        let source = "__codemuxRegister({}, ({send}) => m => {\
            if(m.method==='activate')send('ready',{phase:'activated',registrations:['commands/hello','panels/view']});\
            else if(m.method==='view.mount')send('ui.patch',{viewId:m.params.viewId,records:[[0,'~',{id:'a',type:1,element:'cmx-text',children:[{id:'b',type:3,data:'0'}]},0]]});\
            else if(m.method==='ui.ack')send('ui.patch',{viewId:m.params.viewId,records:[[2,'b',String(m.params.revision)]]});});";
        let running = manager
            .activate(installation, source.into(), false)
            .await
            .unwrap();
        let mut events = manager.events.subscribe();
        let view = show(&manager, &running).await;
        let started = Instant::now();
        let mut applied = Vec::new();
        // Longer than the 10 s window in which repeated excess would stop it.
        while started.elapsed() < Duration::from_secs(11) {
            match tokio::time::timeout(Duration::from_millis(100), events.recv()).await {
                Ok(Ok(UiEvent::Tree { revision, .. })) => {
                    applied.push(Instant::now());
                    manager
                        .acknowledge(&running, &view, revision)
                        .await
                        .unwrap();
                }
                Ok(Ok(UiEvent::Stopped { message, .. })) => panic!("stopped: {message}"),
                _ => {}
            }
        }
        assert!(!running.cancel.is_cancelled());
        assert!(applied.len() >= 200, "only {} batches", applied.len());
        // The host defers batches with a margin, so even arrival times stay
        // within 30 per second.
        for (index, at) in applied.iter().enumerate() {
            let window = applied[index..]
                .iter()
                .take_while(|later| later.duration_since(*at) < Duration::from_secs(1))
                .count();
            assert!(window <= 30, "{window} batches within one second");
        }
        manager.shutdown().await;
    }
    #[test]
    fn ui_backstop_allows_arrival_bunching_and_stops_repeated_excess() {
        let mut rate = Default::default();
        let now = Instant::now();
        // Twice the paced rate arrives at once, then four excess batches are
        // still applied; the fifth excess within 10 s stops the plugin.
        for _ in 0..64 {
            ui_traffic(&mut rate, now).unwrap();
        }
        let error = ui_traffic(&mut rate, now).unwrap_err();
        assert_eq!(error.message, "UI traffic limit");
        // Occasional excess is tolerated: violations expire after 10 s.
        let mut rate = Default::default();
        for second in 0..16 {
            let now = now + Duration::from_secs(second);
            for _ in 0..60 {
                ui_traffic(&mut rate, now).unwrap();
            }
            if second % 3 == 0 {
                assert!(ui_traffic(&mut rate, now).is_ok());
            }
        }
    }
    #[test]
    fn log_ring_keeps_the_newest_entries_within_64_kib() {
        let mut diagnostics = Diagnostics::default();
        for index in 0..1100 {
            diagnostics.record(LogEntry {
                at: 4_102_444_800_000 + index,
                level: "debug",
                bytes: 4096,
            });
        }
        assert_eq!(diagnostics.received, 1100);
        assert_eq!(diagnostics.logs.len(), 1024);
        assert_eq!(diagnostics.logs[0].at, 4_102_444_800_000 + 76);
        // The largest entries (a 13-digit time, the longest level and the
        // largest size) still fit the ring's 64 KiB bound.
        assert!(serde_json::to_vec(&diagnostics.logs).unwrap().len() <= 65536);
    }
    #[test]
    fn supervision_uses_the_specified_delays() {
        let timing = Timing::default();
        assert_eq!(timing.idle, Duration::from_secs(60));
        assert_eq!(timing.request("storage.set"), Duration::from_secs(15));
        assert_eq!(timing.request("http.fetch"), Duration::from_secs(30));
    }
    #[tokio::test]
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_stalled_host_request_times_out_with_a_stable_error() {
        let root = tempfile::tempdir().unwrap();
        let timing = Timing {
            request: Duration::from_millis(300),
            ..Timing::default()
        };
        let manager = Manager::open_with(root.path().into(), test_host_path(), timing).unwrap();
        let installation = installed(
            Manifest::parse(
                include_bytes!("../../addon-protocol/fixtures/hello.json"),
                None,
            )
            .unwrap(),
        );
        manager.save(&installation).unwrap();
        // No renderer answers the notification, so the broker call stalls.
        let source = "__codemuxRegister({}, ({send}) => m => {\
            if(m.method==='activate')send('ready',{phase:'activated',registrations:['commands/hello']});\
            else if(m.method==='command.execute')send('host.request',{operation:'ui.notify',params:{message:'Synthetic'}},1);\
            else if(!m.method&&m.error)console.error(m.error.data.code);});";
        let running = manager
            .activate(installation, source.into(), false)
            .await
            .unwrap();
        let started = Instant::now();
        running
            .host
            .send("command.execute", json!({"id":"hello"}))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !manager
                .diagnostics(&running.manifest.id)
                .unwrap()
                .logs
                .iter()
                .any(|entry| entry.level == "error" && entry.bytes == "TIMEOUT".len())
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the stalled request is answered with TIMEOUT");
        assert!(started.elapsed() >= Duration::from_millis(300));
        assert!(!running.cancel.is_cancelled());
        assert_eq!(running.requests.available_permits(), 16);
        manager.shutdown().await;
    }
    #[tokio::test]
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_outstanding_host_requests_are_bounded_without_quarantine() {
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
        // Each token becomes one append that waits on the host UI. All permits
        // are held, so the plugin reports the seventeenth reply as an error log.
        let source = "__codemuxRegister({}, ({send}) => m => {\
            if(m.method==='activate')send('ready',{phase:'activated',registrations:['commands/hello']});\
            else if(m.method==='command.execute')m.params.tokens.forEach((context,i)=>send('host.request',{operation:'composer.appendText',params:{context,text:'x'}},i+1));\
            else if(!m.method&&m.id===17)console.error(m.error.data.code);});";
        let running = manager
            .activate(installation, source.into(), false)
            .await
            .unwrap();
        let composer = uuid::Uuid::new_v4().to_string();
        manager
            .contexts
            .lock()
            .await
            .register_composer(composer.clone(), "project".into())
            .unwrap();
        let workspace = Workspace {
            id: "project".into(),
            name: "Synthetic project".into(),
            root_name: "project".into(),
            location: "local",
            root: root.path().into(),
        };
        let base = manager
            .context_handle(&running, Some(workspace), Some(composer))
            .await
            .unwrap();
        let mut tokens = Vec::new();
        for _ in 0..17 {
            tokens.push(
                manager
                    .contexts
                    .lock()
                    .await
                    .interact(&base, running.generation(), Instant::now())
                    .unwrap(),
            );
        }
        let mut events = manager.events.subscribe();
        running
            .host
            .send("command.execute", json!({"id":"hello","tokens":tokens}))
            .await
            .unwrap();
        let mut effects = 0;
        tokio::time::timeout(Duration::from_secs(2), async {
            while effects < 16 {
                if let Ok(UiEvent::Effect { .. }) = events.recv().await {
                    effects += 1;
                }
            }
        })
        .await
        .expect("sixteen requests reach the broker");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let diagnostics = manager.diagnostics(&running.manifest.id).unwrap();
                if diagnostics
                    .logs
                    .iter()
                    .any(|entry| entry.level == "error" && entry.bytes == "RESOURCE_LIMIT".len())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the seventeenth request is answered with RESOURCE_LIMIT");
        assert!(!running.cancel.is_cancelled());
        assert_eq!(effects, 16);
        manager.shutdown().await;
    }
    #[tokio::test]
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_log_diagnostics_are_sanitized_and_outlive_the_generation() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().into(), test_host_path()).unwrap();
        let installation = installed(
            Manifest::parse(
                include_bytes!("../../addon-protocol/fixtures/hello.json"),
                None,
            )
            .unwrap(),
        );
        manager.save(&installation).unwrap();
        let source = "__codemuxRegister({}, ({send}) => m => {if(m.method!=='activate')return;\
            console.warn('private response body');console.info('\u{e9}'.repeat(2000));\
            send('ready',{phase:'activated',registrations:['commands/hello']});});";
        let running = manager
            .activate(installation, source.into(), false)
            .await
            .unwrap();
        manager
            .stop(&running.manifest.id, Some("Synthetic failure".into()))
            .await;
        let diagnostics = manager.diagnostics(&running.manifest.id).unwrap();
        assert_eq!(diagnostics.received, 2);
        let entries: Vec<_> = diagnostics
            .logs
            .iter()
            .map(|entry| (entry.level, entry.bytes))
            .collect();
        // Actual UTF-8 sizes after the host's 1,024-character truncation.
        assert_eq!(entries, [("warn", 21), ("info", 2048)]);
        let serialized = serde_json::to_string(&diagnostics).unwrap();
        assert!(!serialized.contains("private"));
    }
    #[tokio::test]
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_ninth_activation_evicts_only_the_least_recently_used_idle_host() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().into(), test_host_path()).unwrap();
        let mut hosts = Vec::new();
        for slot in 'a'..='h' {
            let installation = panel_plugin(&format!("example.slot-{slot}"));
            manager.save(&installation).unwrap();
            hosts.push(
                manager
                    .activate(installation, PANEL_SOURCE.into(), false)
                    .await
                    .unwrap(),
            );
        }
        for running in &hosts[2..] {
            show(&manager, running).await;
        }
        // Both remaining hosts are idle; the older one was used more recently.
        let context = manager.context_handle(&hosts[0], None, None).await.unwrap();
        manager
            .execute(&hosts[0], "hello", "commands", &context)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        let ninth = panel_plugin("example.slot-i");
        manager.save(&ninth).unwrap();
        let ninth = manager
            .activate(ninth, PANEL_SOURCE.into(), false)
            .await
            .unwrap();
        assert!(hosts[1].stopped.is_cancelled());
        assert!(matches!(
            manager.installation(&hosts[1].manifest.id).unwrap().status,
            Status::EnabledIdle
        ));
        assert!(!hosts[0].cancel.is_cancelled());
        // No host is idle: the remaining one shows a view and another is
        // still activating. A further activation reports the limit.
        manager.stop(&ninth.manifest.id, None).await;
        show(&manager, &hosts[0]).await;
        let pending = panel_plugin("example.slot-j");
        manager.save(&pending).unwrap();
        let activating = tokio::spawn({
            let manager = manager.clone();
            // Registers only when released, so it stays mid-activation.
            let source = "__codemuxRegister({}, ({send}) => m => {if(m.method==='command.execute')send('ready',{phase:'activated',registrations:['commands/hello','panels/view']});});";
            async move { manager.activate(pending, source.into(), false).await }
        });
        let pending = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Some(running) = manager.running.lock().await.get("example.slot-j").cloned() {
                    break running;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("activation starts");
        let tenth = panel_plugin("example.slot-k");
        manager.save(&tenth).unwrap();
        let error = manager
            .activate(tenth, PANEL_SOURCE.into(), false)
            .await
            .err()
            .expect("no idle host to evict");
        assert_eq!(error.data.code, ErrorCode::ResourceLimit);
        assert!(!pending.cancel.is_cancelled());
        pending
            .host
            .send("command.execute", json!({"id":"hello"}))
            .await
            .unwrap();
        activating.await.unwrap().unwrap();
        manager.shutdown().await;
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
        let callback = ["Plugin host stopped: Plugin callback failed"];
        let deadline = ["Plugin host stopped: Plugin CPU deadline exceeded"];
        // The drain deadline trips either between jobs or inside one.
        let microtasks = [
            "Plugin host stopped: Microtask deadline exceeded",
            deadline[0],
        ];
        for (code, reasons) in [
            ("throw Error('private failure details')", &callback[..]),
            ("while(true){}", &deadline[..]),
            (
                "Promise.resolve().then(function loop(){Promise.resolve().then(loop)})",
                &microtasks[..],
            ),
            ("function f(){f()} f()", &callback[..]),
            ("new ArrayBuffer(128*1024*1024)", &callback[..]),
            // Ask only the child to exit: the manager has no pending stop and
            // must treat the resulting EOF as an unexpected runtime failure.
            ("", &["Plugin pipe closed", "Plugin host exited"][..]),
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
            let failed = manager.installation(&manifest.id).unwrap();
            assert!(matches!(failed.status, Status::FailedDisabled));
            // The fixed host reason is recorded; plugin text never is.
            let failure = failed.failure.unwrap();
            assert!(reasons.contains(&failure.as_str()), "{code}: {failure}");
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
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_update_candidate_failures_restore_the_previous_tuple_and_report_the_cause() {
        let healthy = "__codemuxRegister({}, ({send}) => m => {if(m.method==='activate')send('ready',{phase:'activated',registrations:['commands/hello']});});";
        let throws = "__codemuxRegister({}, () => m => {if(m.method==='activate')throw Error('candidate failed');});";
        let mismatch = "__codemuxRegister({}, ({send}) => m => {if(m.method==='activate')send('ready',{phase:'activated',registrations:['commands/other']});});";
        // The probe records itself in the candidate generation, so only the
        // normal activation after the registry switch fails.
        let after_switch = "__codemuxRegister({}, ({send}) => m => {if(m.method==='activate')send('host.request',{operation:'storage.get',params:{scope:'global',key:'probed'}},1);else if(m.id===1&&!m.method){if(m.result===true)throw Error('second activation failed');send('host.request',{operation:'storage.set',params:{scope:'global',key:'probed',value:true}},2);}else if(m.id===2&&!m.method)send('ready',{phase:'activated',registrations:['commands/hello']});});";
        for (candidate, expected, replace) in [
            (throws, None, false),
            (
                mismatch,
                Some("Plugin registration does not match its manifest"),
                false,
            ),
            (after_switch, None, false),
            (throws, None, true),
        ] {
            let root = tempfile::tempdir().unwrap();
            let manager = Manager::open(root.path().join("private"), test_host_path()).unwrap();
            let reviews = super::super::lifecycle::Reviews::default();
            let file = root.path().join("healthy.cmxaddon");
            std::fs::write(
                &file,
                super::super::package::fixture_archive_with_source(healthy.as_bytes()),
            )
            .unwrap();
            let review = reviews.prepare_local(&manager, &file).unwrap();
            let old = reviews
                .accept(&manager, &review.token, true, false)
                .await
                .unwrap();
            assert!(old.desired_enabled);
            let package = super::super::lifecycle::tests::package_with_source(
                candidate.as_bytes(),
                |manifest| manifest.version = "2.0.0".into(),
            );
            let source = if replace {
                Source::Local {
                    identity: uuid::Uuid::new_v4().to_string(),
                }
            } else {
                old.source.clone()
            };
            let review = reviews.prepare(&manager, package, source).unwrap();
            // A same-source update keeps the enabled choice; a replacement
            // must ask for it explicitly.
            let error = reviews
                .accept(&manager, &review.token, replace, replace)
                .await
                .err()
                .expect("candidate must fail");
            assert_ne!(error.message, "Plugin did not activate", "specific cause");
            if let Some(expected) = expected {
                assert_eq!(error.message, expected);
            }
            let restored = manager.installation(&old.manifest.id).unwrap();
            assert_eq!(restored.installation_id, old.installation_id);
            assert_eq!(restored.digest, old.digest);
            assert_eq!(restored.data_generation, old.data_generation);
            assert_eq!(restored.grant.as_ref().unwrap().digest, old.digest);
            assert!(restored.desired_enabled, "prior enablement is restored");
            assert!(matches!(restored.status, Status::EnabledIdle));
            assert!(restored.failure.is_none());
            assert_eq!(
                std::fs::read_dir(root.path().join("private/recovery"))
                    .unwrap()
                    .count(),
                0
            );
            assert!(manager.running.lock().await.is_empty());
            manager.ensure_active(&old.manifest.id).await.unwrap();
            manager.shutdown().await;
        }
    }
    #[tokio::test]
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_restored_settings_reach_the_probe_and_the_running_add_on() {
        // Activation fails unless the host serves the restored choice.
        let source = "__codemuxRegister({}, ({send}) => m => {if(m.method==='activate')send('host.request',{operation:'settings.get',params:{}},1);else if(m.id===1&&!m.method){if(m.result['include-files']!==false)throw Error('manifest defaults instead of restored settings');send('ready',{phase:'activated',registrations:['commands/hello']});}});";
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().join("private"), test_host_path()).unwrap();
        let reviews = super::super::lifecycle::Reviews::default();
        let file = root.path().join("settings.cmxaddon");
        let package =
            super::super::lifecycle::tests::package_with_source(source.as_bytes(), |manifest| {
                manifest
                    .settings
                    .push(codemux_addon_protocol::manifest::Setting::Boolean {
                        id: "include-files".into(),
                        label: "Include changed filenames".into(),
                        default: true,
                    })
            });
        std::fs::write(&file, &package.archive).unwrap();
        let review = reviews.prepare_local(&manager, &file).unwrap();
        let old = reviews
            .accept(&manager, &review.token, false, false)
            .await
            .unwrap();
        let chosen = json!({"include-files": false});
        manager
            .set_settings(&old.manifest.id, chosen.clone())
            .await
            .unwrap();
        manager.remove(&old.manifest.id, true).await.unwrap();
        let review = reviews.prepare_local(&manager, &file).unwrap();
        assert!(review.retained_data.is_some());
        let restored = reviews
            .accept_with_data(&manager, &review.token, true, false, true)
            .await
            .expect("probe and activation see the restored settings");
        let running = manager.running.lock().await.get(&old.manifest.id).cloned();
        let running = running.expect("the restored add-on is running");
        assert!(!running.probe);
        assert_eq!(running.installation_id, restored.installation_id);
        assert_eq!(*running.settings.lock().unwrap(), chosen);
        manager.shutdown().await;
    }
    #[tokio::test]
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_paused_installs_and_updates_start_normally_after_resume() {
        let healthy = "__codemuxRegister({}, ({send}) => m => {if(m.method==='activate')send('ready',{phase:'activated',registrations:['commands/hello']});});";
        let throws = "__codemuxRegister({}, () => m => {if(m.method==='activate')throw Error('candidate failed');});";
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().join("private"), test_host_path()).unwrap();
        // An incompatible record beside it never blocks the healthy one.
        let mut future = Manifest::parse(
            include_bytes!("../../addon-protocol/fixtures/hello.json"),
            None,
        )
        .unwrap();
        future.id = "example.future".into();
        future.api = "^2.0.0".into();
        manager.save(&installed(future)).unwrap();
        let reviews = super::super::lifecycle::Reviews::default();
        let source = Source::Local {
            identity: uuid::Uuid::new_v4().to_string(),
        };
        manager.pause_all().await;
        let package =
            super::super::lifecycle::tests::package_with_source(healthy.as_bytes(), |_| {});
        let review = reviews.prepare(&manager, package, source.clone()).unwrap();
        let first = reviews
            .accept(&manager, &review.token, true, false)
            .await
            .unwrap();
        assert!(
            manager.running.lock().await.is_empty(),
            "no probe while paused"
        );
        manager.resume().unwrap();
        // The unclean-exit notice does not outlive a successful start.
        let mut noticed = manager.installation(&first.manifest.id).unwrap();
        noticed.failure = Some("CodeMux closed while this add-on was starting.".into());
        manager.save(&noticed).unwrap();
        let running = manager.ensure_active(&first.manifest.id).await.unwrap();
        assert!(!running.probe);
        let started = manager.installation(&first.manifest.id).unwrap();
        assert!(matches!(started.status, Status::EnabledRunning));
        assert!(started.failure.is_none());
        assert!(matches!(
            manager.installation("example.future").unwrap().status,
            Status::IncompatibleDisabled
        ));
        // A paused update saves the choice; its first real start can fail, and
        // the recorded rollback brings back the working release.
        manager.pause_all().await;
        let package = super::super::lifecycle::tests::package_with_source(throws.as_bytes(), |m| {
            m.version = "2.0.0".into()
        });
        let review = reviews.prepare(&manager, package, source).unwrap();
        let update = reviews
            .accept(&manager, &review.token, false, false)
            .await
            .unwrap();
        assert!(update.desired_enabled, "an update keeps the enabled choice");
        manager.resume().unwrap();
        assert!(manager.ensure_active(&first.manifest.id).await.is_err());
        let failed = manager.installation(&first.manifest.id).unwrap();
        assert!(matches!(failed.status, Status::FailedDisabled));
        assert_eq!(failed.previous.as_ref().unwrap().digest, first.digest);
        manager.rollback(&first.manifest.id).await.unwrap();
        let rolled = manager.installation(&first.manifest.id).unwrap();
        assert_eq!(rolled.digest, first.digest);
        assert!(matches!(rolled.status, Status::EnabledRunning));
        assert!(rolled.failure.is_none());
        manager.shutdown().await;
    }
    #[tokio::test]
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_a_catalog_block_stops_the_running_add_on() {
        use super::super::catalog::tests::{catalog, serve};
        let healthy = "__codemuxRegister({}, ({send}) => m => {if(m.method==='activate')send('ready',{phase:'activated',registrations:['commands/hello']});});";
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().join("private"), test_host_path()).unwrap();
        let reviews = super::super::lifecycle::Reviews::default();
        let package =
            super::super::lifecycle::tests::package_with_source(healthy.as_bytes(), |_| {});
        serve(&manager, Some(&catalog(1, &[&package])), &[&package]);
        let review = reviews
            .prepare_catalog(&manager, "example.hello")
            .await
            .unwrap();
        reviews
            .accept(&manager, &review.token, true, false)
            .await
            .unwrap();
        let running = manager.running.lock().await.get("example.hello").cloned();
        let running = running.expect("the installed add-on is running");
        let mut blocked = catalog(2, &[&package]);
        blocked
            .blocked
            .push(codemux_addon_protocol::catalog::Blocked {
                plugin_id: Some("example.hello".into()),
                sha256: None,
                reason: "Withdrawn".into(),
                date: "2026-09-19T00:00:00Z".into(),
            });
        serve(&manager, Some(&blocked), &[]);
        manager.browse(true).await.unwrap();
        assert!(manager.running.lock().await.is_empty());
        assert!(running.cancel.is_cancelled());
        let current = manager.installation("example.hello").unwrap();
        assert!(matches!(current.status, Status::BlockedDisabled));
        assert_eq!(current.failure.as_deref(), Some("Catalog block: Withdrawn"));
        assert!(manager.ensure_active("example.hello").await.is_err());
        manager.shutdown().await;
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
            .context_handle(&running, Some(workspace.clone()), Some(composer.clone()))
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
        // The composer action and the palette command reach the same broker;
        // each execution carries the interaction its effect consumes.
        for (command, kind, expected) in [
            ("insert", "composerActions", "composer.appendText"),
            ("open", "commands", "panels.open"),
        ] {
            let context = manager
                .context_handle(&running, Some(workspace.clone()), Some(composer.clone()))
                .await
                .unwrap();
            manager
                .execute(&running, command, kind, &context)
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
                            assert_eq!(operation, expected);
                            assert_eq!(params["workspaceId"], "test-workspace");
                            assert_eq!(params["composerId"], composer);
                            if kind == "composerActions" {
                                let text = params["text"].as_str().unwrap();
                                assert!(text.contains("Native test project"));
                                assert!(text.contains("example.txt"));
                            } else {
                                assert_eq!(params["id"], "brief");
                            }
                            manager
                                .claim_effect(&request_id, &generation)
                                .await
                                .unwrap();
                            manager
                                .effect_result(&request_id, &generation, Ok(json!(2)))
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
            .unwrap_or_else(|_| panic!("{kind} {command} should reach the broker"));
        }
        assert!(!running.cancel.is_cancelled());
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
            labeled(nodes, "Add to draft")
        }
        fn labeled(
            nodes: &[codemux_addon_protocol::ui::Node],
            label: &str,
        ) -> Option<(String, String)> {
            for node in nodes {
                if node.element.as_deref() == Some("cmx-button")
                    && content(node) == label
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
                if let Some(found) = labeled(&node.children, label) {
                    return Some(found);
                }
            }
            None
        }
        let mut final_view = None;
        for (status,body,headers,expected) in [
            (401,"{}".to_owned(),std::collections::BTreeMap::new(),"did not accept"),
            (429,"{}".to_owned(),std::collections::BTreeMap::new(),"rate limit"),
            // The primary unauthenticated limit answers 403, not 429.
            (403,"{}".to_owned(),std::collections::BTreeMap::from([("x-ratelimit-remaining".to_owned(),"0".to_owned())]),"rate limit"),
            (200,"[]".to_owned(),std::collections::BTreeMap::new(),"No open issues"),
            (200,json!([{"number":7,"title":"Synthetic issue title","html_url":"https://github.com/synthetic-owner/fixture-repository/issues/7"}]).to_string(),std::collections::BTreeMap::new(),"Synthetic issue title"),
        ] {
            running.http.recorded_responses(vec![super::super::http::Response{status,headers,body}]).await;
            let context=manager.context_handle(&running,Some(workspace.clone()),Some(composer.clone())).await.unwrap();let view=manager.mount(&running,"issues","panels",&context).await.unwrap();
            let callback=tokio::time::timeout(Duration::from_secs(5),async { loop { match events.recv().await.unwrap() {
                UiEvent::Tree{tree,revision,view_id,..} if view_id==view => {manager.acknowledge(&running,&view,revision).await.unwrap();if tree.children.iter().map(content).collect::<String>().contains(expected){break (find_button(&tree.children),labeled(&tree.children,"Open in browser"))}},
                UiEvent::Stopped{message,..}=>panic!("Issue Companion stopped: {message}"),
                UiEvent::Effect{..}=>panic!("Rendering may not insert text or open links"),
                _=>{}
            }}}).await.expect("Issue Companion should render recorded broker result");
            if let (Some(callback),open)=callback { final_view=Some((view,callback,open)); } else { manager.unmount(&running,&view).await.unwrap(); }
        }
        let observed = running.http.observed_requests().await;
        assert_eq!(observed.len(), 5);
        for request in observed {
            assert_eq!(request["origin"], "https://api.github.com");
            assert_eq!(
                request["path"],
                "/repos/synthetic-owner/fixture-repository/issues?state=open&per_page=20"
            );
            assert_eq!(request["method"], "GET");
            assert_eq!(request["body"], Value::Null);
            assert_eq!(request["credentialAttached"], false);
        }
        let (view, (node, callback), open) = final_view.expect("Loaded issue has explicit action");
        manager
            .ui_event(&running, &view, &node, "press", &callback, Value::Null)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5),async {loop{match events.recv().await.unwrap(){
            UiEvent::Effect{request_id,generation,operation,params,..}=>{assert_eq!(operation,"composer.appendText");assert_eq!(params["composerId"],composer);assert_eq!(params["text"],"Synthetic issue title\nhttps://github.com/synthetic-owner/fixture-repository/issues/7");manager.claim_effect(&request_id,&generation).await.unwrap();manager.effect_result(&request_id,&generation,Ok(json!(1))).await.unwrap();break},
            UiEvent::Tree{revision,..}=>manager.acknowledge(&running,&view,revision).await.unwrap(),
            UiEvent::Stopped{message,..}=>panic!("Issue Companion stopped: {message}"),_=>{}
        }}}).await.unwrap();
        // Waits for the next app effect, acknowledging renders of `views`.
        async fn effect(manager: &Manager, running: &Running, events: &mut broadcast::Receiver<UiEvent>, views: &[&str]) -> (String, String, String, Value) {
            tokio::time::timeout(Duration::from_secs(5), async { loop { match events.recv().await.unwrap() {
                UiEvent::Effect{request_id,generation,operation,params,..}=>break (request_id,generation,operation,params),
                UiEvent::Tree{revision,view_id,..} if views.contains(&view_id.as_str())=>manager.acknowledge(running,&view_id,revision).await.unwrap(),
                UiEvent::Stopped{message,..}=>panic!("Issue Companion stopped: {message}"),_=>{}
            }}}).await.expect("Issue Companion effect")
        }
        // Opening the selected issue is an attributed external link.
        let (node, callback) = open.expect("Loaded issue can be opened");
        manager.ui_event(&running, &view, &node, "press", &callback, Value::Null).await.unwrap();
        let (request_id, generation, operation, params) = effect(&manager, &running, &mut events, &[&view]).await;
        assert_eq!(operation, "links.open");
        assert_eq!(params["url"], "https://github.com/synthetic-owner/fixture-repository/issues/7");
        assert_eq!(params["workspaceId"], "issue-fixture");
        manager.claim_effect(&request_id, &generation).await.unwrap();
        manager.effect_result(&request_id, &generation, Ok(Value::Null)).await.unwrap();
        // The composer action opens the accessory bound to that composer.
        let context = manager.context_handle(&running, Some(workspace.clone()), Some(composer.clone())).await.unwrap();
        manager.execute(&running, "browse", "composerActions", &context).await.unwrap();
        let (request_id, generation, operation, params) = effect(&manager, &running, &mut events, &[&view]).await;
        assert_eq!(operation, "composerViews.open");
        assert_eq!(params["id"], "issues");
        assert_eq!(params["composerId"], composer);
        manager.claim_effect(&request_id, &generation).await.unwrap();
        manager.effect_result(&request_id, &generation, Ok(Value::Null)).await.unwrap();
        // The accessory renders the list inside that composer and inserts into it.
        running.http.recorded_responses(vec![super::super::http::Response{status:200,headers:Default::default(),body:json!([{"number":8,"title":"Accessory issue","html_url":"https://github.com/synthetic-owner/fixture-repository/issues/8"}]).to_string()}]).await;
        let context = manager.context_handle(&running, Some(workspace.clone()), Some(composer.clone())).await.unwrap();
        let accessory = manager.mount(&running, "issues", "composerViews", &context).await.unwrap();
        let (node, callback) = tokio::time::timeout(Duration::from_secs(5), async { loop { match events.recv().await.unwrap() {
            UiEvent::Tree{tree,revision,view_id,..} if view_id==accessory => {manager.acknowledge(&running,&accessory,revision).await.unwrap();if tree.children.iter().map(content).collect::<String>().contains("Accessory issue"){if let Some(found)=find_button(&tree.children){break found}}},
            UiEvent::Tree{revision,view_id,..} if view_id==view => manager.acknowledge(&running,&view,revision).await.unwrap(),
            UiEvent::Stopped{message,..}=>panic!("Issue Companion stopped: {message}"),
            UiEvent::Effect{..}=>panic!("Rendering may not insert text or open links"),
            _=>{}
        }}}).await.expect("Accessory renders the recorded issue");
        manager.ui_event(&running, &accessory, &node, "press", &callback, Value::Null).await.unwrap();
        let (request_id, generation, operation, params) = effect(&manager, &running, &mut events, &[&view, &accessory]).await;
        assert_eq!(operation, "composer.appendText");
        assert_eq!(params["composerId"], composer);
        assert_eq!(params["text"], "Accessory issue\nhttps://github.com/synthetic-owner/fixture-repository/issues/8");
        manager.claim_effect(&request_id, &generation).await.unwrap();
        manager.effect_result(&request_id, &generation, Ok(json!(2))).await.unwrap();
        let observed = running.http.observed_requests().await;
        assert_eq!(observed.len(), 6);
        assert!(observed.iter().all(|r| r["path"] == "/repos/synthetic-owner/fixture-repository/issues?state=open&per_page=20" && r["credentialAttached"] == false));
        manager.stop(&installed.manifest.id, None).await;
        assert!(manager.running.lock().await.is_empty());
    }
    fn broker_manifest(id: &str, permissions: &[Permission]) -> Manifest {
        use codemux_addon_protocol::manifest::View as Declared;
        let mut manifest = Manifest::parse(
            include_bytes!("../../addon-protocol/fixtures/hello.json"),
            None,
        )
        .unwrap();
        manifest.id = id.into();
        manifest.permissions = permissions.to_vec();
        manifest.contributes.panels.push(Declared {
            id: "panel".into(),
            title: "Panel".into(),
            icon: "file-text".into(),
        });
        manifest.contributes.composer_views.push(Declared {
            id: "accessory".into(),
            title: "Accessory".into(),
            icon: "file-text".into(),
        });
        manifest
    }
    /// A raw child registering broker_manifest's contributions. `body` runs
    /// for every later message `m`; `store` writes the plugin's private storage.
    fn broker_source(body: &str) -> String {
        format!(
            "__codemuxRegister({{}}, ({{send}}) => {{ let n = 0; const store = (key, value) => send('host.request', {{operation: 'storage.set', params: {{scope: 'global', key, value}}}}, ++n); return m => {{ if (m.method === 'activate') {{ send('ready', {{phase: 'activated', registrations: ['commands/hello', 'panels/panel', 'composerViews/accessory']}}); return; }} {body} }}; }});"
        )
    }
    async fn start(manager: &Arc<Manager>, manifest: Manifest, body: &str) -> Arc<Running> {
        let installation = installed(manifest);
        manager.save(&installation).unwrap();
        manager
            .activate(installation, broker_source(body), false)
            .await
            .unwrap()
    }
    async fn stored(manager: &Manager, running: &Running, key: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let value = manager
                    .request(running, "storage.get", json!({"scope":"global","key":key}))
                    .await
                    .unwrap();
                if !value.is_null() {
                    break value;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("plugin did not store {key}"))
    }
    fn code<T: std::fmt::Debug>(result: Result<T>) -> ErrorCode {
        result.unwrap_err().data.code
    }
    fn project(id: &str, root: &std::path::Path) -> Workspace {
        std::fs::create_dir_all(root).unwrap();
        Workspace {
            id: id.into(),
            name: format!("Project {id}"),
            root_name: "project".into(),
            location: "local",
            root: root.into(),
        }
    }
    fn patch(running: &Running, view: &str, records: Value) -> codemux_addon_protocol::wire::Envelope {
        serde_json::from_value(json!({"jsonrpc":"2.0","generation":running.generation(),"method":"ui.patch","params":{"viewId":view,"records":records}})).unwrap()
    }
    async fn next_effect(events: &mut broadcast::Receiver<UiEvent>) -> (String, String, String, Value) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let UiEvent::Effect {
                    request_id,
                    generation,
                    operation,
                    params,
                    ..
                } = events.recv().await.unwrap()
                {
                    break (request_id, generation, operation, params);
                }
            }
        })
        .await
        .expect("an app effect")
    }
    async fn complete(manager: &Manager, request: &str, generation: &str, value: Value) {
        manager.claim_effect(request, generation).await.unwrap();
        manager
            .effect_result(request, generation, Ok(value))
            .await
            .unwrap();
    }
    #[tokio::test]
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_authority_matrix_reports_stable_error_codes() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().join("addons"), test_host_path()).unwrap();
        let workspace = project("project", &root.path().join("project"));
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(&workspace.root)
            .status()
            .unwrap()
            .success());
        let composer = uuid::Uuid::new_v4().to_string();
        manager
            .contexts
            .lock()
            .await
            .register_composer(composer.clone(), workspace.id.clone())
            .unwrap();
        let denied = start(&manager, broker_manifest("example.denied", &[]), "").await;
        let granted = start(
            &manager,
            broker_manifest(
                "example.granted",
                &[
                    Permission::WorkspaceRead,
                    Permission::GitRead,
                    Permission::ComposerAppend,
                    Permission::ExternalOpen,
                ],
            ),
            "",
        )
        .await;
        let now = Instant::now();
        let expired_at = now.checked_sub(Duration::from_secs(11)).unwrap();
        // Undeclared capability, with a valid context and a live interaction.
        let base = manager
            .context_handle(&denied, Some(workspace.clone()), Some(composer.clone()))
            .await
            .unwrap();
        let click = manager
            .contexts
            .lock()
            .await
            .interact(&base, denied.generation(), now)
            .unwrap();
        for (operation, params) in [
            ("workspace.current", json!({"context":base})),
            ("git.summary", json!({"context":base})),
            ("composer.appendText", json!({"context":click,"text":"x"})),
            ("links.open", json!({"context":click,"url":"https://example.com"})),
            ("panels.open", json!({"id":"undeclared","context":click})),
            ("composerViews.open", json!({"id":"panel","context":click})),
        ] {
            assert_eq!(
                code(manager.request(&denied, operation, params).await),
                ErrorCode::PermissionDenied,
                "{operation}"
            );
        }
        // Undeclared methods and author-supplied identity or paths.
        for (operation, params) in [
            ("invoke", json!({"command":"run_shell"})),
            ("storage.get", json!({"scope":"global","key":"k","pluginId":"example.granted"})),
            ("storage.get", json!({"scope":"example.granted","key":"k"})),
            ("workspace.current", json!({"context":base,"workspaceId":"other"})),
            ("git.summary", json!({"context":base,"path":"/"})),
            ("http.fetch", json!({"context":base,"origin":"https://api.example.com","path":"/","method":"GET","url":"https://127.0.0.1"})),
        ] {
            assert_eq!(
                code(manager.request(&granted, operation, params).await),
                ErrorCode::InvalidMessage,
                "{operation}"
            );
        }
        // Forged handles and handles issued to another plugin generation.
        let own = manager
            .context_handle(&granted, Some(workspace.clone()), Some(composer.clone()))
            .await
            .unwrap();
        for handle in ["forged", base.as_str(), click.as_str()] {
            for (operation, params) in [
                ("workspace.current", json!({"context":handle})),
                ("git.summary", json!({"context":handle})),
                ("storage.get", json!({"scope":"workspace","context":handle,"key":"k"})),
                ("composer.appendText", json!({"context":handle,"text":"x"})),
            ] {
                assert_eq!(
                    code(manager.request(&granted, operation, params).await),
                    ErrorCode::ContextStale,
                    "{operation} with {handle}"
                );
            }
        }
        // Base handles and expired interactions cannot affect the app...
        let expired = manager
            .contexts
            .lock()
            .await
            .interact(&own, granted.generation(), expired_at)
            .unwrap();
        for (operation, params) in [
            ("composer.appendText", json!({"context":own,"text":"x"})),
            ("composer.appendText", json!({"context":expired,"text":"x"})),
            ("links.open", json!({"context":expired,"url":"https://example.com"})),
            ("panels.open", json!({"id":"panel","context":own})),
            ("composerViews.open", json!({"id":"accessory","context":expired})),
        ] {
            assert_eq!(
                code(manager.request(&granted, operation, params).await),
                ErrorCode::InteractionRequired,
                "{operation}"
            );
        }
        // ...but the context of an expired interaction still reads its project.
        let current = manager
            .request(&granted, "workspace.current", json!({"context":expired}))
            .await
            .unwrap();
        assert_eq!(
            current,
            json!({"id":"project","name":"Project project","rootName":"project","location":"local"})
        );
        manager
            .request(&granted, "git.summary", json!({"context":expired}))
            .await
            .unwrap();
        // Checks that precede an effect do not spend the interaction.
        let click = manager
            .contexts
            .lock()
            .await
            .interact(&own, granted.generation(), Instant::now())
            .unwrap();
        for (operation, params, expected) in [
            ("links.open", json!({"context":click,"url":"http://example.com"}), ErrorCode::NetworkDenied),
            ("http.fetch", json!({"context":click,"origin":"https://api.example.com","path":"/","method":"GET"}), ErrorCode::NetworkDenied),
            ("composerViews.open", json!({"id":"undeclared","context":click}), ErrorCode::PermissionDenied),
            ("composer.appendText", json!({"context":click,"text":"x".repeat(32769)}), ErrorCode::InvalidMessage),
        ] {
            assert_eq!(
                code(manager.request(&granted, operation, params).await),
                expected,
                "{operation}"
            );
        }
        // One interaction authorizes exactly one effect.
        let mut events = manager.events.subscribe();
        let broker = manager.clone();
        let instance = granted.clone();
        let interaction = click.clone();
        let append = tokio::spawn(async move {
            broker
                .request(&instance, "composer.appendText", json!({"context":interaction,"text":"once"}))
                .await
        });
        let (request, generation, operation, params) = next_effect(&mut events).await;
        assert_eq!(operation, "composer.appendText");
        assert_eq!(params["composerId"], composer);
        assert_eq!(params["workspaceId"], "project");
        complete(&manager, &request, &generation, json!(3)).await;
        assert_eq!(append.await.unwrap().unwrap(), json!(3));
        assert_eq!(
            code(
                manager
                    .request(&granted, "links.open", json!({"context":click,"url":"https://example.com"}))
                    .await
            ),
            ErrorCode::InteractionRequired
        );
        manager
            .request(&granted, "workspace.current", json!({"context":click}))
            .await
            .unwrap();
        // A panel bound to no composer explains why it cannot insert text.
        let unbound = manager
            .context_handle(&granted, Some(workspace.clone()), None)
            .await
            .unwrap();
        let mut reasons = Vec::new();
        for register in [false, true] {
            if register {
                manager
                    .contexts
                    .lock()
                    .await
                    .register_composer(uuid::Uuid::new_v4().to_string(), workspace.id.clone())
                    .unwrap();
            }
            let click = manager
                .contexts
                .lock()
                .await
                .interact(&unbound, granted.generation(), Instant::now())
                .unwrap();
            let error = manager
                .request(&granted, "composer.appendText", json!({"context":click,"text":"x"}))
                .await
                .unwrap_err();
            assert_eq!(error.data.code, ErrorCode::NoComposer);
            reasons.push(error.message);
        }
        assert_eq!(reasons[0], "No chat composer is available");
        assert!(reasons[1].starts_with("Several chat composers are open"));
        // Three notifications per minute; the fourth is refused.
        for _ in 0..3 {
            let broker = manager.clone();
            let instance = granted.clone();
            let notify = tokio::spawn(async move {
                broker
                    .request(&instance, "ui.notify", json!({"message":"Synthetic notice"}))
                    .await
            });
            let (request, generation, operation, _) = next_effect(&mut events).await;
            assert_eq!(operation, "ui.notify");
            complete(&manager, &request, &generation, Value::Null).await;
            notify.await.unwrap().unwrap();
        }
        assert_eq!(
            code(
                manager
                    .request(&granted, "ui.notify", json!({"message":"Synthetic notice"}))
                    .await
            ),
            ErrorCode::ResourceLimit
        );
        assert!(!denied.cancel.is_cancelled() && !granted.cancel.is_cancelled());
        manager.shutdown().await;
    }
    #[tokio::test]
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_workspace_storage_is_bound_to_the_project_root_not_its_reusable_id() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().join("addons"), test_host_path()).unwrap();
        let running = start(&manager, broker_manifest("example.storage", &[]), "").await;
        let first = root.path().join("first");
        let second = root.path().join("second");
        async fn access(
            manager: &Manager,
            running: &Running,
            workspace: Workspace,
            value: Option<&str>,
        ) -> Value {
            let context = manager
                .context_handle(running, Some(workspace), None)
                .await
                .unwrap();
            let (operation, mut params) = match value {
                Some(_) => ("storage.set", json!({"scope":"workspace","context":context,"key":"brief"})),
                None => ("storage.get", json!({"scope":"workspace","context":context,"key":"brief"})),
            };
            if let Some(value) = value {
                params["value"] = json!(value);
            }
            manager.request(running, operation, params).await.unwrap()
        }
        access(&manager, &running, project("workspace-1", &first), Some("first project")).await;
        // After the project closes, a restart may hand its counter ID to another project.
        manager.change_workspace(None).await;
        assert_eq!(
            access(&manager, &running, project("workspace-1", &second), None).await,
            Value::Null
        );
        access(&manager, &running, project("workspace-1", &second), Some("second project")).await;
        // Reopening the first project under a new ID finds its own data.
        assert_eq!(
            access(&manager, &running, project("workspace-7", &first), None).await,
            "first project"
        );
        assert_eq!(
            access(&manager, &running, project("workspace-9", &second), None).await,
            "second project"
        );
        let scope = super::super::workspace::storage_scope(&project("workspace-1", &first));
        assert!(!scope.contains("first") && !scope.contains("workspace-1"));
        manager.shutdown().await;
    }
    #[tokio::test]
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_commands_and_failed_mounts_leave_no_context_handles() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().join("addons"), test_host_path()).unwrap();
        let workspace = project("project", &root.path().join("project"));
        let running = start(
            &manager,
            broker_manifest("example.handles", &[Permission::WorkspaceRead]),
            "if (m.method === 'command.execute') store('context', m.params.context);",
        )
        .await;
        let handles = || async { manager.contexts.lock().await.handle_count() };
        let before = handles().await;
        for _ in 0..3 {
            let context = manager
                .context_handle(&running, Some(workspace.clone()), None)
                .await
                .unwrap();
            manager
                .execute(&running, "hello", "commands", &context)
                .await
                .unwrap();
        }
        assert_eq!(handles().await, before + 3, "only each interaction remains");
        // The plugin received the execution handle itself as its interaction.
        let received = stored(&manager, &running, "context").await;
        manager
            .request(&running, "workspace.current", json!({"context":received}))
            .await
            .unwrap();
        // A command or view that cannot start revokes the handle it was given.
        for kind in ["commands", "composerActions", "unknown"] {
            let context = manager
                .context_handle(&running, Some(workspace.clone()), None)
                .await
                .unwrap();
            let id = if kind == "commands" { "missing" } else { "hello" };
            assert!(manager.execute(&running, id, kind, &context).await.is_err());
            let context = manager
                .context_handle(&running, Some(workspace.clone()), None)
                .await
                .unwrap();
            assert!(manager.mount(&running, "missing", "panels", &context).await.is_err());
        }
        assert_eq!(handles().await, before + 3);
        let mut views = Vec::new();
        for _ in 0..limits::VIEWS {
            let context = manager
                .context_handle(&running, Some(workspace.clone()), None)
                .await
                .unwrap();
            views.push(manager.mount(&running, "panel", "panels", &context).await.unwrap());
        }
        let context = manager
            .context_handle(&running, Some(workspace.clone()), None)
            .await
            .unwrap();
        assert_eq!(
            code(manager.mount(&running, "panel", "panels", &context).await),
            ErrorCode::ResourceLimit
        );
        assert_eq!(handles().await, before + 3 + limits::VIEWS);
        // The four mounted views remain usable after the refused fifth.
        for view in &views {
            manager
                .message(&running, patch(&running, view, json!([[0,"~",{"id":"text","type":3,"data":"still live"},0]])))
                .await
                .unwrap();
        }
        for view in &views {
            manager.unmount(&running, view).await.unwrap();
        }
        assert_eq!(handles().await, before + 3);
        assert!(!running.cancel.is_cancelled());
        manager.shutdown().await;
    }
    #[tokio::test]
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_workspace_changes_give_contexts_only_to_workspace_readers() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().join("addons"), test_host_path()).unwrap();
        let body = "if (m.method === 'workspace.changed') store('changed', {context: m.params.context});";
        let reader = start(
            &manager,
            broker_manifest("example.reader", &[Permission::WorkspaceRead]),
            body,
        )
        .await;
        let other = start(
            &manager,
            broker_manifest("example.other", &[Permission::GitRead]),
            body,
        )
        .await;
        let context = manager.context_handle(&other, None, None).await.unwrap();
        let view = manager
            .mount(&other, "panel", "panels", &context)
            .await
            .unwrap();
        manager
            .change_workspace(Some(project("project", &root.path().join("project"))))
            .await;
        let received = stored(&manager, &reader, "changed").await;
        assert!(received["context"].is_string());
        assert_eq!(
            manager
                .request(&reader, "workspace.current", json!({"context":received["context"]}))
                .await
                .unwrap()["id"],
            "project"
        );
        // Every plugin still learns to unmount its views, without a context.
        assert_eq!(stored(&manager, &other, "changed").await, json!({"context":null}));
        assert!(other.views.lock().await.is_empty());
        assert!(manager.unmount(&other, &view).await.is_ok());
        assert_eq!(manager.contexts.lock().await.handle_count(), 1);
        manager.shutdown().await;
    }
    #[tokio::test]
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_claimed_effect_can_report_after_its_interaction_deadline() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().join("addons"), test_host_path()).unwrap();
        let running = start(
            &manager,
            broker_manifest("example.effects", &[Permission::ComposerAppend]),
            "",
        )
        .await;
        let workspace = project("project", &root.path().join("project"));
        let composer = uuid::Uuid::new_v4().to_string();
        manager
            .contexts
            .lock()
            .await
            .register_composer(composer.clone(), workspace.id.clone())
            .unwrap();
        let base = manager
            .context_handle(&running, Some(workspace), Some(composer))
            .await
            .unwrap();
        let mut events = manager.events.subscribe();
        let append = |deadline_in: Duration| {
            let manager = manager.clone();
            let running = running.clone();
            let base = base.clone();
            async move {
                // The interaction was issued so that its 10 s window ends soon.
                let issued = Instant::now() + deadline_in - Duration::from_secs(10);
                let click = manager
                    .contexts
                    .lock()
                    .await
                    .interact(&base, running.generation(), issued)
                    .unwrap();
                tokio::spawn(async move {
                    manager
                        .request(&running, "composer.appendText", json!({"context":click,"text":"late"}))
                        .await
                })
            }
        };
        // Claimed just in time: the draft changed, so the plugin gets the revision.
        let task = append(Duration::from_millis(200)).await;
        let (request, generation, ..) = next_effect(&mut events).await;
        manager.claim_effect(&request, &generation).await.unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        manager
            .effect_result(&request, &generation, Ok(json!(9)))
            .await
            .unwrap();
        assert_eq!(task.await.unwrap().unwrap(), json!(9));
        // Unclaimed at the deadline: nothing may be applied afterwards.
        let task = append(Duration::from_millis(200)).await;
        let (request, generation, ..) = next_effect(&mut events).await;
        assert_eq!(
            code(tokio::time::timeout(Duration::from_secs(2), task).await.unwrap().unwrap()),
            ErrorCode::Timeout
        );
        assert!(manager.claim_effect(&request, &generation).await.is_err());
        assert!(manager
            .effect_result(&request, &generation, Ok(json!(1)))
            .await
            .is_err());
        // A claim extends the wait by a bounded grace period only.
        let task = append(Duration::from_millis(100)).await;
        let (request, generation, ..) = next_effect(&mut events).await;
        manager.claim_effect(&request, &generation).await.unwrap();
        let claimed = Instant::now();
        assert_eq!(
            code(tokio::time::timeout(Duration::from_secs(4), task).await.unwrap().unwrap()),
            ErrorCode::Timeout
        );
        assert!(claimed.elapsed() >= CLAIMED_GRACE - Duration::from_millis(50));
        assert!(manager
            .effect_result(&request, &generation, Ok(json!(1)))
            .await
            .is_err());
        assert!(manager.effects.lock().await.is_empty());
        manager.shutdown().await;
    }
    fn http_manifest(id: &str) -> Manifest {
        let mut manifest = broker_manifest(id, &[]);
        manifest.http.push(codemux_addon_protocol::manifest::HttpGrant {
            origin: "https://api.example.com".into(),
            methods: vec![codemux_addon_protocol::manifest::HttpMethod::GET],
            credential: None,
        });
        manifest
    }
    const FETCH_ON_COMMAND: &str = "if (m.method === 'command.execute') send('host.request', {operation: 'http.fetch', params: {context: m.params.context, origin: 'https://api.example.com', path: '/data', method: 'GET'}}, 900); else if (!m.method && m.id === 900) store('result', m.error ? m.error.data.code : m.result.status);";
    #[tokio::test]
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_http_result_too_large_to_frame_fails_promptly() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().join("addons"), test_host_path()).unwrap();
        let running = start(&manager, http_manifest("example.large"), FETCH_ON_COMMAND).await;
        // Under the 512 KiB body limit, but every byte escapes to six.
        running
            .http
            .recorded_responses(vec![super::super::http::Response {
                status: 200,
                headers: Default::default(),
                body: "\u{1}".repeat(300 * 1024),
            }])
            .await;
        let context = manager
            .context_handle(&running, Some(project("project", &root.path().join("project"))), None)
            .await
            .unwrap();
        let started = Instant::now();
        manager
            .execute(&running, "hello", "commands", &context)
            .await
            .unwrap();
        assert_eq!(stored(&manager, &running, "result").await, "RESOURCE_LIMIT");
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!running.cancel.is_cancelled());
        assert_eq!(running.http.available_slots(), 4);
        manager.shutdown().await;
    }
    #[tokio::test]
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_disable_or_project_switch_during_http_releases_the_request() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().join("addons"), test_host_path()).unwrap();
        let workspace = project("project", &root.path().join("project"));
        let installation = installed(http_manifest("example.slow"));
        manager.save(&installation).unwrap();
        let running = manager
            .activate(installation.clone(), broker_source(FETCH_ON_COMMAND), false)
            .await
            .unwrap();
        running.http.recorded_delay(Duration::from_secs(10));
        let late = super::super::http::Response {
            status: 200,
            headers: Default::default(),
            body: "late project data".into(),
        };
        running
            .http
            .recorded_responses(vec![late.clone(), late])
            .await;
        let in_flight = |count: usize| {
            let running = running.clone();
            async move {
                tokio::time::timeout(Duration::from_secs(2), async {
                    while running.http.observed_requests().await.len() < count {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                })
                .await
                .expect("request in flight");
            }
        };
        // Switching projects disposes the context and cancels its request.
        let context = manager
            .context_handle(&running, Some(workspace.clone()), None)
            .await
            .unwrap();
        let broker = manager.clone();
        let instance = running.clone();
        let fetch = tokio::spawn(async move {
            broker
                .request(&instance, "http.fetch", json!({"context":context,"origin":"https://api.example.com","path":"/data","method":"GET"}))
                .await
        });
        in_flight(1).await;
        assert_eq!(running.http.available_slots(), 3);
        manager.change_workspace(None).await;
        let error = tokio::time::timeout(Duration::from_secs(2), fetch)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(matches!(
            error.data.code,
            ErrorCode::PluginStopped | ErrorCode::ContextStale
        ));
        assert_eq!(running.http.available_slots(), 4);
        // Disabling abandons the child's request; nothing late is delivered.
        let mut events = manager.events.subscribe();
        let context = manager
            .context_handle(&running, Some(workspace), None)
            .await
            .unwrap();
        manager
            .execute(&running, "hello", "commands", &context)
            .await
            .unwrap();
        in_flight(2).await;
        assert_eq!(running.requests.available_permits(), 15);
        let mut disabled = manager.installation(&running.manifest.id).unwrap();
        disabled.desired_enabled = false;
        disabled.status = Status::InstalledDisabled;
        manager.save(&disabled).unwrap();
        let started = Instant::now();
        manager.stop(&running.manifest.id, None).await;
        assert!(running.stopped.is_cancelled());
        tokio::time::timeout(Duration::from_secs(2), async {
            while running.requests.available_permits() < 16 || running.http.available_slots() < 4 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("request permit and HTTP slot are released");
        assert!(started.elapsed() < Duration::from_secs(2));
        tokio::time::sleep(Duration::from_millis(100)).await;
        while let Ok(event) = events.try_recv() {
            assert!(
                !matches!(event, UiEvent::Tree { .. } | UiEvent::Effect { .. }),
                "a stopped generation delivered UI work"
            );
        }
        assert!(matches!(
            manager.installation(&running.manifest.id).unwrap().status,
            Status::InstalledDisabled
        ));
        // Re-enabling starts a fresh generation.
        let mut enabled = manager.installation(&running.manifest.id).unwrap();
        enabled.desired_enabled = true;
        enabled.status = Status::EnabledIdle;
        manager.save(&enabled).unwrap();
        let next = manager
            .activate(enabled, broker_source(""), false)
            .await
            .unwrap();
        assert_ne!(next.generation(), running.generation());
        manager.shutdown().await;
    }
    #[tokio::test]
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_saved_credentials_that_cannot_be_read_are_never_skipped() {
        use super::super::credentials::RecordedStore;
        let root = tempfile::tempdir().unwrap();
        let mut manager = Manager::open(root.path().join("addons"), test_host_path()).unwrap();
        let backend = Arc::new(RecordedStore::default());
        Arc::get_mut(&mut manager).unwrap().credentials = Credentials::recorded(backend.clone());
        let mut manifest = http_manifest("example.credential");
        manifest.http[0].credential = Some("token".into());
        manifest
            .credentials
            .push(codemux_addon_protocol::manifest::Credential {
                id: "token".into(),
                label: "API token".into(),
                origin: "https://api.example.com".into(),
                kind: codemux_addon_protocol::manifest::CredentialType::Bearer,
            });
        let running = start(&manager, manifest, "").await;
        let ok = super::super::http::Response {
            status: 200,
            headers: Default::default(),
            body: "{}".into(),
        };
        running
            .http
            .recorded_responses(vec![ok.clone(), ok.clone(), ok])
            .await;
        let context = manager
            .context_handle(&running, Some(project("project", &root.path().join("project"))), None)
            .await
            .unwrap();
        let fetch = json!({"context":context,"origin":"https://api.example.com","path":"/data","method":"GET"});
        // Never configured: v1 credentials are optional; the request is unauthenticated.
        manager
            .request(&running, "http.fetch", fetch.clone())
            .await
            .unwrap();
        let installation = manager.installation(&running.manifest.id).unwrap();
        manager
            .save_credential(&installation, "token", "synthetic-token".into(), false)
            .await
            .unwrap();
        manager
            .request(&running, "http.fetch", fetch.clone())
            .await
            .unwrap();
        // Saved but unreadable: fail rather than silently drop authentication.
        backend
            .locked
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let error = manager
            .request(&running, "http.fetch", fetch)
            .await
            .unwrap_err();
        assert_eq!(error.data.code, ErrorCode::CredentialRequired);
        assert!(!error.message.contains("synthetic-token"));
        let attached: Vec<_> = running
            .http
            .observed_requests()
            .await
            .iter()
            .map(|r| r["credentialAttached"].clone())
            .collect();
        assert_eq!(attached, [json!(false), json!(true)]);
        manager.shutdown().await;
    }
    fn buttons(count: usize) -> Value {
        let children = (0..count)
            .map(|i| json!({"id":format!("b{i}"),"type":1,"element":"cmx-button","eventListeners":{"press":{"callbackId":format!("c{i}")}}}))
            .collect::<Vec<_>>();
        json!([[0,"~",{"id":"root","type":1,"element":"cmx-stack","children":children},0]])
    }
    #[tokio::test]
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_live_callbacks_are_limited_per_plugin_not_per_view() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().join("addons"), test_host_path()).unwrap();
        let running = start(&manager, broker_manifest("example.callbacks", &[]), "").await;
        let mut views = Vec::new();
        for _ in 0..limits::VIEWS {
            let context = manager.context_handle(&running, None, None).await.unwrap();
            views.push(manager.mount(&running, "panel", "panels", &context).await.unwrap());
        }
        // Three views already hold 1,350 callbacks each, each valid alone.
        // Committing them directly keeps the measured batch small.
        for view in &views[..3] {
            let tree = Tree {
                children: vec![serde_json::from_value(buttons(1350)[0][2].clone()).unwrap()],
            };
            tree.validate().unwrap();
            running.views.lock().await.get_mut(view).unwrap().tree = tree;
        }
        let last = &views[3];
        let error = manager
            .message(&running, patch(&running, last, buttons(47)))
            .await
            .unwrap_err();
        assert_eq!(error.data.code, ErrorCode::ResourceLimit);
        assert!(running.views.lock().await[last].tree.children.is_empty());
        // Exactly 4,096 live callbacks are allowed.
        manager
            .message(&running, patch(&running, last, buttons(46)))
            .await
            .unwrap();
        manager.acknowledge(&running, last, 1).await.unwrap();
        let one_more = json!([[0,"root",{"id":"extra","type":1,"element":"cmx-button","eventListeners":{"press":{"callbackId":"extra"}}},0]]);
        assert_eq!(
            code(manager.message(&running, patch(&running, last, one_more.clone())).await),
            ErrorCode::ResourceLimit
        );
        // Unmounting a view releases its callbacks.
        manager.unmount(&running, &views[0]).await.unwrap();
        manager
            .message(&running, patch(&running, last, one_more))
            .await
            .unwrap();
        assert_eq!(running.views.lock().await[last].tree.callback_count(), 47);
        manager.shutdown().await;
    }
    #[tokio::test]
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_ui_traffic_limits_stop_only_the_offending_plugin() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().join("addons"), test_host_path()).unwrap();
        let healthy = start(&manager, broker_manifest("example.healthy", &[]), "").await;
        let text = |i: usize| json!([[0,"~",{"id":format!("t{i}"),"type":3,"data":"x"},0]]);
        // The trusted renderer holds at most two unacknowledged batches.
        let queued = start(&manager, broker_manifest("example.queued", &[]), "").await;
        let context = manager.context_handle(&queued, None, None).await.unwrap();
        let view = manager.mount(&queued, "panel", "panels", &context).await.unwrap();
        manager.message(&queued, patch(&queued, &view, text(0))).await.unwrap();
        manager.acknowledge(&queued, &view, 1).await.unwrap();
        for i in 1..3 {
            manager.message(&queued, patch(&queued, &view, text(i))).await.unwrap();
        }
        let error = manager
            .message(&queued, patch(&queued, &view, text(3)))
            .await
            .unwrap_err();
        assert_eq!(error.data.code, ErrorCode::ResourceLimit);
        assert_eq!(queued.views.lock().await[&view].revision, 3);
        // The trusted host paces batches to 30/s. This backstop applies every
        // acknowledged batch and faults only on repeated excess over twice that.
        let busy = start(&manager, broker_manifest("example.busy", &[]), "").await;
        let context = manager.context_handle(&busy, None, None).await.unwrap();
        let view = manager.mount(&busy, "panel", "panels", &context).await.unwrap();
        let mut refused = None;
        for i in 0..80 {
            // Pace acknowledgements so the child's bounded call queue keeps up.
            tokio::time::sleep(Duration::from_millis(2)).await;
            match manager.message(&busy, patch(&busy, &view, text(i))).await {
                Ok(()) => {
                    let revision = busy.views.lock().await[&view].revision;
                    manager.acknowledge(&busy, &view, revision).await.unwrap();
                }
                Err(error) => {
                    refused = Some(error);
                    break;
                }
            }
        }
        assert_eq!(refused.expect("repeated excess").data.code, ErrorCode::ResourceLimit);
        let applied = busy.views.lock().await[&view].revision;
        assert!((60..80).contains(&applied), "applied {applied} batches");
        // A real child flooding the pipe is stopped; the other plugin continues.
        let flood = start(
            &manager,
            broker_manifest("example.flood", &[]),
            "if (m.method === 'view.mount') for (let i = 0; i < 40; i++) send('ui.patch', {viewId: m.params.viewId, records: [[0, '~', {id: 't' + i, type: 3, data: 'x'}, 0]]});",
        )
        .await;
        let context = manager.context_handle(&flood, None, None).await.unwrap();
        manager.mount(&flood, "panel", "panels", &context).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), flood.stopped.cancelled())
            .await
            .expect("flooding generation is stopped");
        assert!(matches!(
            manager.installation(&flood.manifest.id).unwrap().status,
            Status::FailedDisabled
        ));
        assert!(!healthy.cancel.is_cancelled());
        manager
            .request(&healthy, "storage.set", json!({"scope":"global","key":"alive","value":true}))
            .await
            .unwrap();
        manager.shutdown().await;
    }
    #[tokio::test]
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_markdown_links_need_markdown_permission_and_https() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().join("addons"), test_host_path()).unwrap();
        let workspace = project("project", &root.path().join("project"));
        let markdown = json!([
            [0,"~",{"id":"doc","type":1,"element":"cmx-markdown","children":[{"id":"body","type":3,"data":"[Issue](https://example.com/issue)"}]},0],
            [0,"~",{"id":"label","type":1,"element":"cmx-text","children":[{"id":"plain","type":3,"data":"text"}]},1]
        ]);
        let mut mounted = Vec::new();
        for (id, permissions) in [
            ("example.linker", vec![Permission::ExternalOpen]),
            ("example.plain", vec![]),
        ] {
            let running = start(&manager, broker_manifest(id, &permissions), "").await;
            let context = manager
                .context_handle(&running, Some(workspace.clone()), None)
                .await
                .unwrap();
            let view = manager.mount(&running, "panel", "panels", &context).await.unwrap();
            manager
                .message(&running, patch(&running, &view, markdown.clone()))
                .await
                .unwrap();
            mounted.push((running, view));
        }
        let (linker, view) = mounted[0].clone();
        let (plain, plain_view) = mounted[1].clone();
        for (running, view, node, url, expected) in [
            (&linker, view.as_str(), "label", "https://example.com", ErrorCode::ContextStale),
            (&linker, view.as_str(), "missing", "https://example.com", ErrorCode::ContextStale),
            (&linker, "view-99", "doc", "https://example.com", ErrorCode::ContextStale),
            (&plain, plain_view.as_str(), "doc", "https://example.com", ErrorCode::PermissionDenied),
            (&linker, view.as_str(), "doc", "http://example.com", ErrorCode::NetworkDenied),
            (&linker, view.as_str(), "doc", "javascript:alert(1)", ErrorCode::NetworkDenied),
        ] {
            assert_eq!(
                code(manager.ui_link(running, view, node, url).await),
                expected,
                "{node} {url}"
            );
        }
        // A real click opens the link only through an attributed app effect.
        let mut events = manager.events.subscribe();
        let broker = manager.clone();
        let instance = linker.clone();
        let target = view.clone();
        let click = tokio::spawn(async move {
            broker
                .ui_link(&instance, &target, "doc", "https://example.com/issue")
                .await
        });
        let (request, generation, operation, params) = next_effect(&mut events).await;
        assert_eq!(operation, "links.open");
        assert_eq!(params["url"], "https://example.com/issue");
        assert_eq!(params["workspaceId"], "project");
        complete(&manager, &request, &generation, Value::Null).await;
        click.await.unwrap().unwrap();
        assert!(!linker.cancel.is_cancelled() && !plain.cancel.is_cancelled());
        manager.shutdown().await;
    }
    #[tokio::test]
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_stale_ui_events_are_refused_without_stopping_the_plugin() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().join("addons"), test_host_path()).unwrap();
        let running = start(
            &manager,
            broker_manifest("example.events", &[]),
            "if (m.method === 'ui.event') store('event', m.params.callbackId);",
        )
        .await;
        let workspace = project("project", &root.path().join("project"));
        let composer = uuid::Uuid::new_v4().to_string();
        manager
            .contexts
            .lock()
            .await
            .register_composer(composer.clone(), workspace.id.clone())
            .unwrap();
        let context = manager
            .context_handle(&running, Some(workspace), Some(composer.clone()))
            .await
            .unwrap();
        let view = manager.mount(&running, "panel", "panels", &context).await.unwrap();
        manager
            .message(&running, patch(&running, &view, json!([[0,"~",{"id":"b","type":1,"element":"cmx-button","eventListeners":{"press":{"callbackId":"one"}}},0]])))
            .await
            .unwrap();
        manager.acknowledge(&running, &view, 1).await.unwrap();
        // The plugin replaced the listener; the old callback is released.
        manager
            .message(&running, patch(&running, &view, json!([[3,"b","press",{"callbackId":"two"},3]])))
            .await
            .unwrap();
        for (view, node, event, callback) in [
            (view.as_str(), "b", "press", "one"),
            (view.as_str(), "missing", "press", "two"),
            (view.as_str(), "b", "change", "two"),
            ("view-99", "b", "press", "two"),
        ] {
            assert_eq!(
                code(manager.ui_event(&running, view, node, event, callback, Value::Null).await),
                ErrorCode::ContextStale,
                "{node} {event} {callback}"
            );
        }
        manager
            .ui_event(&running, &view, "b", "press", "two", Value::Null)
            .await
            .unwrap();
        assert_eq!(stored(&manager, &running, "event").await, "two");
        // A closed composer or an unmounted view makes its callbacks stale.
        manager.contexts.lock().await.revoke_composer(&composer);
        assert_eq!(
            code(manager.ui_event(&running, &view, "b", "press", "two", Value::Null).await),
            ErrorCode::ContextStale
        );
        manager.unmount(&running, &view).await.unwrap();
        assert_eq!(
            code(manager.ui_event(&running, &view, "b", "press", "two", Value::Null).await),
            ErrorCode::ContextStale
        );
        assert!(!running.cancel.is_cancelled());
        assert!(matches!(
            manager.installation(&running.manifest.id).unwrap().status,
            Status::EnabledRunning
        ));
        manager.shutdown().await;
    }
    #[tokio::test]
    #[ignore = "Requires the independently built host; run scripts/addons/test-native.sh"]
    async fn native_unmount_always_reaches_the_child_and_late_batches_are_inert() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().join("addons"), test_host_path()).unwrap();
        let running = start(
            &manager,
            broker_manifest("example.views", &[]),
            "if (m.method === 'view.unmount') store('unmounted', m.params.viewId);",
        )
        .await;
        let paced = || async {
            while running.host.outstanding().await >= 8 {
                assert!(!running.cancel.is_cancelled(), "the host stopped");
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        };
        // More disposals than the former 4,096-entry tombstone set. Project
        // switches dispose four views per message; pacing keeps the child's
        // share of its wall-clock CPU budget low on a loaded machine.
        for _ in 0..1025 {
            for _ in 0..limits::VIEWS {
                paced().await;
                let context = manager.context_handle(&running, None, None).await.unwrap();
                manager.mount(&running, "panel", "panels", &context).await.unwrap();
            }
            manager.change_workspace(None).await;
            tokio::time::sleep(Duration::from_millis(4)).await;
        }
        assert!(running.views.lock().await.is_empty());
        assert_eq!(manager.contexts.lock().await.handle_count(), 0);
        // An unmount after all those disposals still reaches the child.
        let context = manager.context_handle(&running, None, None).await.unwrap();
        let view = manager.mount(&running, "panel", "panels", &context).await.unwrap();
        assert_eq!(view, "view-4101");
        manager.unmount(&running, &view).await.unwrap();
        assert_eq!(stored(&manager, &running, "unmounted").await, "view-4101");
        assert_eq!(manager.contexts.lock().await.handle_count(), 0);
        // A batch flushed before the child saw view.unmount is ignored...
        for view in ["view-1", "view-4100"] {
            manager
                .message(&running, patch(&running, view, json!([[0,"~",{"id":"late","type":3,"data":"late"},0]])))
                .await
                .unwrap();
        }
        // ...but a view this generation never issued is a protocol error.
        for view in ["view-4102", "view-01", "view-", "other"] {
            assert_eq!(
                code(
                    manager
                        .message(&running, patch(&running, view, json!([])))
                        .await
                ),
                ErrorCode::InvalidMessage,
                "{view}"
            );
        }
        let context = manager.context_handle(&running, None, None).await.unwrap();
        let view = manager.mount(&running, "panel", "panels", &context).await.unwrap();
        assert_eq!(view, "view-4102");
        manager
            .message(&running, patch(&running, &view, json!([[0,"~",{"id":"live","type":3,"data":"live"},0]])))
            .await
            .unwrap();
        assert!(!running.cancel.is_cancelled());
        manager.shutdown().await;
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
    #[test]
    fn registry_reset_moves_the_damaged_folder_aside_and_opens_empty() {
        let root = tempfile::tempdir().unwrap();
        let addons = root.path().join("addons-v1");
        std::fs::create_dir_all(addons.join("state").join(uuid::Uuid::new_v4().to_string()))
            .unwrap();
        std::fs::write(addons.join("registry.sqlite"), b"not a sqlite database").unwrap();
        assert!(Manager::open(addons.clone(), PathBuf::from("unused")).is_err());
        let backup = Manager::reset_registry(&addons).unwrap();
        assert_eq!(
            std::fs::read(backup.join("registry.sqlite")).unwrap(),
            b"not a sqlite database"
        );
        assert!(backup.join("state").is_dir(), "private data is kept");
        let manager = Manager::open(addons.clone(), PathBuf::from("unused")).unwrap();
        assert!(manager.list().unwrap().is_empty());
        assert!(!manager.paused());
    }
    #[tokio::test]
    async fn unsupported_api_record_is_incompatible_without_breaking_the_registry() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().into(), PathBuf::from("unused")).unwrap();
        let manifest = Manifest::parse(
            include_bytes!("../../addon-protocol/fixtures/hello.json"),
            None,
        )
        .unwrap();
        let healthy = installed(manifest.clone());
        manager.save(&healthy).unwrap();
        let mut future = manifest;
        future.id = "example.future".into();
        future.api = "^2.0.0".into();
        manager.save(&installed(future)).unwrap();
        drop(manager);
        let manager = Manager::open(root.path().into(), PathBuf::from("unused")).unwrap();
        assert_eq!(manager.list().unwrap().len(), 2);
        assert!(matches!(
            manager.installation("example.hello").unwrap().status,
            Status::EnabledIdle
        ));
        let future = manager.installation("example.future").unwrap();
        assert!(matches!(future.status, Status::IncompatibleDisabled));
        assert_eq!(
            future.failure.as_deref(),
            Some("Requires plugin API ^2.0.0; this CodeMux supports 1.0.0")
        );
        assert!(!future.compatibility().compatible);
        assert!(healthy.compatibility().compatible);
        assert!(manager
            .activate(future.clone(), String::new(), false)
            .await
            .is_err());
        // Retained data of an incompatible record is tolerated at startup too.
        manager.remove("example.future", true).await.unwrap();
        drop(manager);
        let manager = Manager::open(root.path().into(), PathBuf::from("unused")).unwrap();
        assert_eq!(manager.list().unwrap().len(), 1);
    }
    #[tokio::test]
    async fn tampered_installed_package_is_refused_before_activation() {
        let root = tempfile::tempdir().unwrap();
        let manager = Manager::open(root.path().into(), PathBuf::from("unused")).unwrap();
        let package = root.path().join("fixture.cmxaddon");
        std::fs::write(&package, super::super::package::fixture_archive()).unwrap();
        let reviews = super::super::lifecycle::Reviews::default();
        let review = reviews.prepare_local(&manager, &package).unwrap();
        let mut installation = reviews
            .accept(&manager, &review.token, false, false)
            .await
            .unwrap();
        installation.desired_enabled = true;
        installation.status = Status::EnabledIdle;
        manager.save(&installation).unwrap();
        let stored = root
            .path()
            .join("packages")
            .join(&installation.manifest.id)
            .join(&installation.digest)
            .join("package.cmxaddon");
        let original = std::fs::read(&stored).unwrap();
        std::fs::write(
            &stored,
            super::super::package::fixture_archive_with_source(b"globalThis.modified = true;"),
        )
        .unwrap();
        let error = manager
            .ensure_active(&installation.manifest.id)
            .await
            .err()
            .unwrap();
        assert_eq!(
            error.message,
            "Package digest does not match the accepted release"
        );
        assert!(manager.running.lock().await.is_empty());
        std::fs::write(&stored, original).unwrap();
        let mut edited = installation.clone();
        edited.manifest.name = "Renamed".into();
        manager.save(&edited).unwrap();
        let error = manager
            .ensure_active(&installation.manifest.id)
            .await
            .err()
            .unwrap();
        assert_eq!(error.message, "Installed manifest was modified");
        assert!(manager.running.lock().await.is_empty());
    }
}
