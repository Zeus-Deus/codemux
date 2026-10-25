//! Supervised plugin subprocesses. This is a reliability/authority boundary, not an OS sandbox.
use super::platform_manifest::{safe_package_path, valid_identifier};
use futures_util::{stream::FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{mpsc, watch, Semaphore},
    task::JoinHandle,
};

const MAX_MESSAGE: usize = 1_048_576;
const MAX_MESSAGES: usize = 10_000;
const MAX_HOST_CALLS: usize = 4096;
pub const PLUGIN_HOST_PATH_ENV: &str = "CODEMUX_PLUGIN_HOST_PATH";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginRuntimeError {
    pub reason: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_effect: Option<Value>,
}
impl PluginRuntimeError {
    pub fn new(reason: &str, message: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            message: message.into().chars().take(2000).collect(),
            code: None,
            prior_effect: None,
        }
    }
}
impl std::fmt::Display for PluginRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.reason, self.message)
    }
}
impl std::error::Error for PluginRuntimeError {}
type Result<T> = std::result::Result<T, PluginRuntimeError>;

#[derive(Clone, Debug)]
pub struct RuntimeCancellation {
    sender: Arc<watch::Sender<bool>>,
}
impl Default for RuntimeCancellation {
    fn default() -> Self {
        Self {
            sender: Arc::new(watch::channel(false).0),
        }
    }
}
impl RuntimeCancellation {
    pub fn cancel(&self) {
        self.sender.send_replace(true);
    }
    pub fn is_cancelled(&self) -> bool {
        *self.sender.borrow()
    }
    pub async fn cancelled(&self) {
        let mut receiver = self.sender.subscribe();
        while !*receiver.borrow_and_update() {
            if receiver.changed().await.is_err() {
                break;
            }
        }
    }
}
struct CancelOnDrop(RuntimeCancellation);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

#[derive(Clone, Debug)]
pub struct PluginExecutionInput {
    pub installation_id: String,
    pub generation: String,
    pub directory: PathBuf,
    pub entry: String,
    pub data_directory: PathBuf,
    pub handler: String,
    pub operation_id: String,
    pub input: Value,
    pub caller: Value,
    pub context: Option<Value>,
    pub capabilities: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct PluginHostCall {
    pub installation_id: String,
    pub generation: String,
    pub invocation_id: String,
    pub operation_id: String,
    pub caller: Value,
    pub context: Option<Value>,
    pub method: String,
    pub input: Value,
    /// Check again at the effect boundary; spawned work must not outlive this signal.
    pub cancellation: RuntimeCancellation,
}
pub type PluginHostCallback = Arc<
    dyn Fn(PluginHostCall) -> Pin<Box<dyn Future<Output = Result<Value>> + Send>> + Send + Sync,
>;

#[derive(Clone, Debug)]
pub struct PluginRuntimeConfig {
    pub executable: Option<PathBuf>,
    pub args: Vec<String>,
    pub startup_timeout: Duration,
    pub invocation_timeout: Duration,
    pub stop_grace: Duration,
    pub restart_backoff: Duration,
    pub max_failures: usize,
}
impl Default for PluginRuntimeConfig {
    fn default() -> Self {
        Self {
            executable: resolve_plugin_host(None),
            args: vec![],
            startup_timeout: Duration::from_secs(10),
            invocation_timeout: Duration::from_secs(30),
            stop_grace: Duration::from_millis(500),
            restart_backoff: Duration::from_secs(1),
            max_failures: 3,
        }
    }
}
pub fn resolve_plugin_host(resources: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(PLUGIN_HOST_PATH_ENV).filter(|s| !s.is_empty()) {
        return Some(path.into());
    }
    let name = format!(
        "codemux-plugin-host-{}{}",
        crate::agent_provider::claude::sidecar_path::target_triple(),
        if cfg!(windows) { ".exe" } else { "" }
    );
    if let Some(resources) = resources {
        let path = resources.join("binaries").join(&name);
        if path.is_file() {
            return Some(path);
        }
    }
    #[cfg(debug_assertions)]
    {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(&name);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}
struct Slot {
    generation: String,
    directory: PathBuf,
    entry: String,
    data_directory: PathBuf,
    capabilities: Vec<String>,
    revoked: RuntimeCancellation,
    failures: AtomicUsize,
    process: tokio::sync::Mutex<Option<WorkerProcess>>,
    permits: Arc<Semaphore>,
}
struct WorkerLease<'a> {
    guard: tokio::sync::MutexGuard<'a, Option<WorkerProcess>>,
    completed: bool,
}
impl std::ops::Deref for WorkerLease<'_> {
    type Target = Option<WorkerProcess>;
    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}
impl std::ops::DerefMut for WorkerLease<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}
impl Drop for WorkerLease<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.guard.take();
        }
    }
}
pub struct PluginSupervisor {
    config: PluginRuntimeConfig,
    enabled: AtomicBool,
    slots: Mutex<HashMap<String, Arc<Slot>>>,
}
impl PluginSupervisor {
    pub fn new(config: PluginRuntimeConfig) -> Self {
        Self {
            config,
            enabled: AtomicBool::new(!super::disabled_by_env()),
            slots: Mutex::new(HashMap::new()),
        }
    }
    fn allowed(&self) -> Result<()> {
        if !self.enabled.load(Ordering::SeqCst) || super::disabled_by_env() {
            Err(PluginRuntimeError::new(
                "disabled",
                "Plugin execution is disabled by safe mode or the global kill switch",
            ))
        } else {
            Ok(())
        }
    }
    fn slot(&self, input: &PluginExecutionInput) -> Result<Arc<Slot>> {
        self.allowed()?;
        if !valid_identifier(&input.installation_id)
            || !valid_identifier(&input.generation)
            || !valid_identifier(&input.operation_id)
            || !input.directory.is_absolute()
            || !input.data_directory.is_absolute()
            || !input.caller.is_object()
        {
            return Err(PluginRuntimeError::new(
                "invalid_input",
                "Invalid host execution identity",
            ));
        }
        let mut slots = self.slots.lock().map_err(|_| {
            PluginRuntimeError::new("unavailable", "Plugin runtime state is unavailable")
        })?;
        let slot = slots
            .entry(input.installation_id.clone())
            .or_insert_with(|| {
                Arc::new(Slot {
                    generation: input.generation.clone(),
                    directory: input.directory.clone(),
                    entry: input.entry.clone(),
                    data_directory: input.data_directory.clone(),
                    capabilities: input.capabilities.clone(),
                    revoked: RuntimeCancellation::default(),
                    failures: AtomicUsize::new(0),
                    process: tokio::sync::Mutex::new(None),
                    permits: Arc::new(Semaphore::new(8)),
                })
            })
            .clone();
        if slot.generation != input.generation
            || slot.revoked.is_cancelled()
            || slot.directory != input.directory
            || slot.entry != input.entry
            || slot.data_directory != input.data_directory
            || slot.capabilities != input.capabilities
        {
            return Err(PluginRuntimeError::new(
                "stale_generation",
                "Plugin generation has been revoked; resume after host admission",
            ));
        }
        Ok(slot)
    }
    /// Identity, enabled intent, manifest compatibility and grants must be admitted by the service
    /// before entry; it must recheck them for every host callback. JS never supplies that identity.
    pub async fn invoke(
        &self,
        input: PluginExecutionInput,
        host: PluginHostCallback,
        cancel: RuntimeCancellation,
    ) -> Result<Value> {
        let slot = self.slot(&input)?;
        let _permit = slot
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| PluginRuntimeError::new("busy", "Plugin invocation queue is full"))?;
        let invocation_cancel = RuntimeCancellation::default();
        let _cancel_guard = CancelOnDrop(invocation_cancel.clone());
        let guard = tokio::select! {biased;
            _=cancel.cancelled()=>return Err(PluginRuntimeError::new("cancelled","Plugin invocation was cancelled")),
            _=slot.revoked.cancelled()=>return Err(PluginRuntimeError::new("stale_generation","Plugin generation was revoked")),
            guard=slot.process.lock()=>guard,
        };
        // Dropping the caller future must also kill a spinning worker, not merely release its lock.
        let mut process = WorkerLease {
            guard,
            completed: false,
        };
        self.allowed()?;
        if slot.revoked.is_cancelled() {
            return Err(PluginRuntimeError::new(
                "stale_generation",
                "Plugin generation was revoked",
            ));
        }
        if slot.failures.load(Ordering::SeqCst) >= self.config.max_failures.max(1) {
            return Err(PluginRuntimeError::new(
                "quarantined",
                "This plugin repeatedly failed. Resume it to retry.",
            ));
        }
        let work = async {
            if process.is_none() {
                let failures = slot.failures.load(Ordering::SeqCst);
                if failures > 0 {
                    tokio::time::sleep(
                        (self
                            .config
                            .restart_backoff
                            .saturating_mul(1u32 << failures.min(5)))
                        .min(Duration::from_secs(30)),
                    )
                    .await;
                }
                self.allowed()?;
                let worker = WorkerProcess::spawn(&self.config, &input).await?;
                *process = Some(worker);
            }
            tokio::time::timeout(
                self.config.invocation_timeout,
                process
                    .as_mut()
                    .ok_or_else(|| {
                        PluginRuntimeError::new("start_failed", "Plugin worker unavailable")
                    })?
                    .invoke(&input, host, invocation_cancel.clone()),
            )
            .await
            .unwrap_or_else(|_| {
                Err(PluginRuntimeError::new(
                    "timeout",
                    "Plugin invocation exceeded its deadline",
                ))
            })
        };
        // This also bounds a host-service future; dropping it revokes its invocation signal.
        let outcome = tokio::select! {biased;
            _=cancel.cancelled()=>Err(PluginRuntimeError::new("cancelled","Plugin invocation was cancelled")),
            _=slot.revoked.cancelled()=>Err(PluginRuntimeError::new("stale_generation","Plugin generation was revoked")),
            result=tokio::time::timeout(self.config.startup_timeout+self.config.invocation_timeout,work)=>result.unwrap_or_else(|_|Err(PluginRuntimeError::new("timeout","Plugin invocation exceeded its deadline"))),
        };
        invocation_cancel.cancel();
        if let Err(error) = &outcome {
            // A normal handler error does not poison process health. Protocol/crash/deadline faults do.
            if ["crashed", "start_failed", "invalid_result", "timeout"]
                .contains(&error.reason.as_str())
            {
                slot.failures.fetch_add(1, Ordering::SeqCst);
            }
            if error.reason != "operation_failed" {
                if let Some(mut worker) = process.take() {
                    worker.stop(self.config.stop_grace).await;
                }
            }
        }
        process.completed = true;
        outcome
    }
    /// Revokes authority immediately, then stops/reaps only this installation's owned process.
    pub async fn stop(&self, installation_id: &str) {
        let slot = self
            .slots
            .lock()
            .ok()
            .and_then(|slots| slots.get(installation_id).cloned());
        if let Some(slot) = slot {
            slot.revoked.cancel();
            let mut process = slot.process.lock().await;
            if let Some(mut worker) = process.take() {
                worker.stop(self.config.stop_grace).await;
            }
        }
    }
    pub async fn resume(&self, installation_id: &str) {
        self.stop(installation_id).await;
        if let Ok(mut slots) = self.slots.lock() {
            slots.remove(installation_id);
        }
    }
    pub async fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
        if !enabled {
            self.stop_all().await;
        }
    }
    async fn stop_all(&self) {
        let slots: Vec<_> = self
            .slots
            .lock()
            .map(|slots| slots.values().cloned().collect())
            .unwrap_or_default();
        for slot in &slots {
            slot.revoked.cancel();
        }
        futures_util::future::join_all(slots.into_iter().map(|slot| async move {
            let mut process = slot.process.lock().await;
            if let Some(mut worker) = process.take() {
                worker.stop(self.config.stop_grace).await;
            }
        }))
        .await;
        if let Ok(mut slots) = self.slots.lock() {
            slots.clear();
        }
    }
    pub async fn shutdown(&self) {
        self.enabled.store(false, Ordering::SeqCst);
        self.stop_all().await;
    }
}
impl Drop for PluginSupervisor {
    fn drop(&mut self) {
        if let Ok(slots) = self.slots.lock() {
            for slot in slots.values() {
                slot.revoked.cancel();
            }
        }
    }
}

struct WorkerProcess {
    child: Child,
    stdin: ChildStdin,
    messages: mpsc::Receiver<Result<Value>>,
    reader: JoinHandle<()>,
    stderr: JoinHandle<()>,
    active_invocation: Option<String>,
    #[cfg(windows)]
    _job: windows_job::OwnedJob,
}
impl WorkerProcess {
    async fn spawn(config: &PluginRuntimeConfig, input: &PluginExecutionInput) -> Result<Self> {
        let executable = config.executable.as_ref().ok_or_else(|| {
            PluginRuntimeError::new(
                "start_failed",
                "Bundled plugin host is unavailable; repair the app installation",
            )
        })?;
        let mut command = Command::new(executable);
        command
            .args(&config.args)
            .current_dir(&input.data_directory)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for key in [
            "PATH",
            "SystemRoot",
            "WINDIR",
            "TMPDIR",
            "TMP",
            "TEMP",
            "LANG",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        // Bun/Node injection options and provider credentials are deliberately not inherited.
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn().map_err(|e| {
            PluginRuntimeError::new(
                "start_failed",
                format!("Cannot launch bundled plugin host: {e}"),
            )
        })?;
        #[cfg(windows)]
        let job = windows_job::OwnedJob::attach(&child)
            .map_err(|e| PluginRuntimeError::new("start_failed", e))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| PluginRuntimeError::new("start_failed", "Worker stdin unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| PluginRuntimeError::new("start_failed", "Worker stdout unavailable"))?;
        let mut stderr_pipe = child
            .stderr
            .take()
            .ok_or_else(|| PluginRuntimeError::new("start_failed", "Worker stderr unavailable"))?;
        let (sender, messages) = mpsc::channel(32);
        let reader = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            for _ in 0..MAX_MESSAGES {
                let message = read_message(&mut reader).await;
                let failed = message.is_err();
                if sender.send(message).await.is_err() || failed {
                    return;
                }
            }
            let _ = sender
                .send(Err(PluginRuntimeError::new(
                    "invalid_result",
                    "Plugin IPC message budget exceeded",
                )))
                .await;
        });
        // Drain rather than accumulate; a chatty stderr cannot hold a pipe or allocate host memory.
        let stderr = tokio::spawn(async move {
            let mut buffer = [0u8; 8192];
            while let Ok(n) = stderr_pipe.read(&mut buffer).await {
                if n == 0 {
                    break;
                }
            }
        });
        let mut worker = Self {
            child,
            stdin,
            messages,
            reader,
            stderr,
            active_invocation: None,
            #[cfg(windows)]
            _job: job,
        };
        let activation = async {
            let boot = worker.next().await?;
            if boot.get("type").and_then(Value::as_str) != Some("booted")
                || boot.get("protocol").and_then(Value::as_u64) != Some(1)
            {
                return Err(PluginRuntimeError::new(
                    "start_failed",
                    "Invalid plugin host boot protocol",
                ));
            }
            worker.send(json!({"type":"init","protocol":1,"directory":input.directory,"entry":input.entry,"dataDirectory":input.data_directory,"capabilities":input.capabilities})).await?;
            let mut logs = 0;
            loop {
                let message = worker.next().await?;
                match message.get("type").and_then(Value::as_str) {
                    Some("ready") if message.get("protocol").and_then(Value::as_u64) == Some(1) => {
                        return Ok(())
                    }
                    Some("log") if valid_log(&message) && logs < 100 => {
                        logs += 1;
                    }
                    // No host API is admitted at import/activation or without a live invocation.
                    _ => {
                        return Err(PluginRuntimeError::new(
                            "start_failed",
                            "Plugin activation failed or attempted an out-of-invocation host call",
                        ))
                    }
                }
            }
        };
        tokio::time::timeout(config.startup_timeout, activation)
            .await
            .map_err(|_| {
                PluginRuntimeError::new("start_failed", "Plugin activation exceeded its deadline")
            })??;
        Ok(worker)
    }
    async fn send(&mut self, value: Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(&value).map_err(|_| {
            PluginRuntimeError::new("invalid_input", "Cannot encode plugin message")
        })?;
        if bytes.len() > MAX_MESSAGE {
            return Err(PluginRuntimeError::new(
                "invalid_input",
                "Plugin IPC message exceeds 1 MiB",
            ));
        }
        bytes.push(b'\n');
        self.stdin
            .write_all(&bytes)
            .await
            .map_err(|_| PluginRuntimeError::new("crashed", "Plugin host disconnected"))
    }
    async fn next(&mut self) -> Result<Value> {
        self.messages
            .recv()
            .await
            .unwrap_or_else(|| Err(PluginRuntimeError::new("crashed", "Plugin worker exited")))
    }
    async fn invoke(
        &mut self,
        input: &PluginExecutionInput,
        host: PluginHostCallback,
        cancellation: RuntimeCancellation,
    ) -> Result<Value> {
        let invocation_id = uuid::Uuid::new_v4().to_string();
        let _revoke_host_calls = CancelOnDrop(cancellation.clone());
        self.active_invocation = Some(invocation_id.clone());
        self.send(json!({"type":"invoke","invocationId":invocation_id,"operationId":input.operation_id,"handler":input.handler,"input":input.input,"caller":input.caller,"context":input.context})).await?;
        let mut calls = HashSet::new();
        let mut logs = 0;
        type HostResponse = Pin<Box<dyn Future<Output = (String, Result<Value>)> + Send>>;
        let mut pending: FuturesUnordered<HostResponse> = FuturesUnordered::new();
        loop {
            // Keep reading results while host calls run. A forgotten promise must lose authority
            // when its handler returns, rather than keeping a host service alive until deadline.
            let message = tokio::select! {biased;
                message=self.next()=>message?,
                Some((call_id,result))=pending.next(),if !pending.is_empty()=>{
                    if cancellation.is_cancelled(){return Err(PluginRuntimeError::new("cancelled","Plugin invocation ended"));}
                    let response=match result {
                        Ok(value)=>json!({"type":"host-result","invocationId":invocation_id,"callId":call_id,"ok":true,"value":value}),
                        Err(error)=>json!({"type":"host-result","invocationId":invocation_id,"callId":call_id,"ok":false,"error":error}),
                    };
                    self.send(response).await?;continue;
                }
            };
            let kind = message.get("type").and_then(Value::as_str);
            if kind == Some("log") && valid_log(&message) && logs < 100 {
                logs += 1;
                continue;
            }
            if message.get("invocationId").and_then(Value::as_str) != Some(&invocation_id) {
                return Err(PluginRuntimeError::new(
                    "invalid_result",
                    "Plugin used an inactive invocation identity",
                ));
            }
            match kind {
                Some("result") => {
                    let output = message.get("result").cloned().ok_or_else(|| {
                        PluginRuntimeError::new("invalid_result", "Plugin result missing")
                    })?;
                    validate_result(&output)?;
                    self.active_invocation = None;
                    return Ok(output);
                }
                Some("failure") => {
                    self.active_invocation = None;
                    return Err(PluginRuntimeError::new(
                        "operation_failed",
                        message
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("Plugin handler failed"),
                    ));
                }
                Some("host-call") => {
                    let call_id = message
                        .get("callId")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty() && s.len() <= 256)
                        .ok_or_else(|| {
                            PluginRuntimeError::new("invalid_result", "Invalid host call identity")
                        })?;
                    let method = message
                        .get("method")
                        .and_then(Value::as_str)
                        .filter(|m| HOST_METHODS.contains(m))
                        .ok_or_else(|| {
                            PluginRuntimeError::new("invalid_result", "Unknown host method")
                        })?;
                    if calls.len() >= MAX_HOST_CALLS
                        || pending.len() >= 128
                        || !calls.insert(call_id.to_owned())
                    {
                        return Err(PluginRuntimeError::new(
                            "invalid_result",
                            "Plugin host call budget or identity violated",
                        ));
                    }
                    if cancellation.is_cancelled() {
                        return Err(PluginRuntimeError::new(
                            "cancelled",
                            "Plugin invocation ended",
                        ));
                    }
                    let request = PluginHostCall {
                        installation_id: input.installation_id.clone(),
                        generation: input.generation.clone(),
                        invocation_id: invocation_id.clone(),
                        operation_id: input.operation_id.clone(),
                        caller: input.caller.clone(),
                        context: input.context.clone(),
                        method: method.into(),
                        input: message.get("input").cloned().unwrap_or(Value::Null),
                        cancellation: cancellation.clone(),
                    };
                    let host = host.clone();
                    let call_id = call_id.to_owned();
                    pending.push(Box::pin(async move { (call_id, host(request).await) }));
                }
                _ => {
                    return Err(PluginRuntimeError::new(
                        "invalid_result",
                        "Invalid plugin IPC response",
                    ))
                }
            }
        }
    }
    async fn stop(&mut self, grace: Duration) {
        if let Some(id) = self.active_invocation.take() {
            let _ =
                tokio::time::timeout(grace, self.send(json!({"type":"cancel","invocationId":id})))
                    .await;
        }
        let _ = tokio::time::timeout(grace, self.send(json!({"type":"stop"}))).await;
        // Keep the leader unreaped until the group has been signalled, preventing PID reuse.
        if !grace.is_zero() {
            tokio::time::sleep(grace.min(Duration::from_secs(1))).await;
        }
        self.kill_tree();
        let _ = tokio::time::timeout(Duration::from_secs(1), self.child.wait()).await;
        self.reader.abort();
        self.stderr.abort();
    }
    fn kill_tree(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.child.id().filter(|p| *p > 1) {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        self._job.terminate();
        let _ = self.child.start_kill();
    }
}
impl Drop for WorkerProcess {
    fn drop(&mut self) {
        self.kill_tree();
        self.reader.abort();
        self.stderr.abort();
    }
}
async fn read_message<R: AsyncRead + Unpin>(reader: &mut BufReader<R>) -> Result<Value> {
    let mut bytes = Vec::new();
    loop {
        let buffer = reader
            .fill_buf()
            .await
            .map_err(|_| PluginRuntimeError::new("crashed", "Plugin IPC read failed"))?;
        if buffer.is_empty() {
            return Err(PluginRuntimeError::new("crashed", "Plugin IPC closed"));
        }
        let count = buffer
            .iter()
            .position(|b| *b == b'\n')
            .map(|n| n + 1)
            .unwrap_or(buffer.len());
        if bytes.len() + count > MAX_MESSAGE + 1 {
            return Err(PluginRuntimeError::new(
                "invalid_result",
                "Plugin IPC frame exceeds 1 MiB",
            ));
        }
        let done = buffer[count - 1] == b'\n';
        bytes.extend_from_slice(&buffer[..count]);
        reader.consume(count);
        if done {
            return serde_json::from_slice(&bytes).map_err(|_| {
                PluginRuntimeError::new("invalid_result", "Malformed plugin IPC JSON")
            });
        }
    }
}
const HOST_METHODS: &[&str] = &[
    "settings.get",
    "secrets.get",
    "storage.get",
    "storage.set",
    "storage.delete",
    "projects.list",
    "workspaces.list",
    "models.list",
    "threads.list",
    "threads.metadata",
    "threads.read",
    "threads.send",
    "threads.create",
    "workflow.query",
    "ui.invalidate",
];
fn valid_log(value: &Value) -> bool {
    value
        .get("level")
        .and_then(Value::as_str)
        .is_some_and(|s| ["debug", "info", "warn", "error"].contains(&s))
        && value
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|s| s.chars().count() <= 2000)
}
fn validate_result(value: &Value) -> Result<()> {
    let Some(object) = value.as_object() else {
        return Err(PluginRuntimeError::new(
            "invalid_result",
            "Plugin result must be an object",
        ));
    };
    if serde_json::to_vec(value).map_or(true, |b| b.len() > 128_000)
        || value
            .get("message")
            .is_some_and(|v| !v.as_str().is_some_and(|s| s.chars().count() <= 16_000))
        || value
            .get("intents")
            .is_some_and(|v| !v.as_array().is_some_and(|a| a.len() <= 20))
        || value
            .get("images")
            .is_some_and(|v| !v.as_array().is_some_and(|a| a.len() <= 4))
        || object.keys().any(|k| {
            ![
                "message",
                "data",
                "card",
                "intents",
                "images",
                "unsupportedIntentCount",
            ]
            .contains(&k.as_str())
        })
    {
        return Err(PluginRuntimeError::new(
            "invalid_result",
            "Plugin result violates the public output contract",
        ));
    }
    let text = |v: &Value, max: usize| v.as_str().is_some_and(|s| s.chars().count() <= max);
    let id = |v: &Value| v.as_str().is_some_and(valid_identifier);
    if let Some(card) = value.get("card") {
        if !card.as_object().is_some_and(|o| {
            o.keys()
                .all(|k| ["viewId", "summary", "data"].contains(&k.as_str()))
        }) || !id(&card["viewId"])
            || !text(&card["summary"], 1000)
            || card["summary"].as_str() == Some("")
            || serde_json::to_vec(card).map_or(true, |v| v.len() > 4096)
            || card
                .get("data")
                .is_some_and(|v| serde_json::to_vec(v).map_or(true, |v| v.len() > 2048))
        {
            return Err(PluginRuntimeError::new(
                "invalid_result",
                "Invalid plugin conversation card",
            ));
        }
    }
    if let Some(intents) = value.get("intents").and_then(Value::as_array) {
        for intent in intents {
            let valid = match intent.get("type").and_then(Value::as_str) {
                Some("composer.insert") => {
                    text(&intent["text"], 64_000)
                        && intent.get("quote").is_none_or(Value::is_boolean)
                }
                Some("preview.open") => intent["url"].as_str().is_some_and(|s| {
                    s.len() <= 4096
                        && url::Url::parse(s).is_ok_and(|u| ["http", "https"].contains(&u.scheme()))
                }),
                Some("thread.reveal") => {
                    id(&intent["threadId"]) && intent.get("messageId").is_none_or(|v| text(v, 256))
                }
                Some("view.open") => id(&intent["viewId"]),
                _ => false,
            };
            if !valid {
                return Err(PluginRuntimeError::new(
                    "invalid_result",
                    "Invalid plugin client intent",
                ));
            }
        }
    }
    if let Some(images) = value.get("images").and_then(Value::as_array) {
        for image in images {
            if !image
                .get("mimeType")
                .and_then(Value::as_str)
                .is_some_and(|s| ["image/png", "image/jpeg"].contains(&s))
                || !image.get("path").and_then(Value::as_str).is_some_and(|s| {
                    safe_package_path(
                        Path::new(if cfg!(windows) { "C:/data" } else { "/data" }),
                        s,
                    )
                    .is_ok()
                })
            {
                return Err(PluginRuntimeError::new(
                    "invalid_result",
                    "Invalid plugin image reference",
                ));
            }
        }
    }
    if value
        .get("unsupportedIntentCount")
        .is_some_and(|v| !v.is_u64())
    {
        return Err(PluginRuntimeError::new(
            "invalid_result",
            "Invalid unsupported intent count",
        ));
    }
    // Domain/client validators additionally check declared view IDs, tree membership and image bytes.
    Ok(())
}

// Job handles own Windows descendants even if a child exits or shutdown is interrupted. Assignment
// happens before init, so plugin code cannot spawn outside the job during activation.
#[cfg(windows)]
pub(super) mod windows_job {
    use super::*;
    type Handle = *mut std::ffi::c_void;
    #[repr(C)]
    struct Basic {
        per_process: i64,
        per_job: i64,
        flags: u32,
        min: usize,
        max: usize,
        active: u32,
        affinity: usize,
        priority: u32,
        scheduling: u32,
    }
    #[repr(C)]
    struct IoCounters {
        read_ops: u64,
        write_ops: u64,
        other_ops: u64,
        read_bytes: u64,
        write_bytes: u64,
        other_bytes: u64,
    }
    #[repr(C)]
    struct Extended {
        basic: Basic,
        io: IoCounters,
        process_memory: usize,
        job_memory: usize,
        peak_process: usize,
        peak_job: usize,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attributes: *const std::ffi::c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(
            job: Handle,
            class: i32,
            info: *const std::ffi::c_void,
            length: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn TerminateJobObject(job: Handle, code: u32) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }
    pub struct OwnedJob(Handle);
    unsafe impl Send for OwnedJob {}
    impl OwnedJob {
        pub fn attach(child: &Child) -> std::result::Result<Self, String> {
            let raw = child
                .raw_handle()
                .ok_or("Worker process handle unavailable")?;
            let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if job.is_null() {
                return Err("Cannot create plugin lifecycle job".into());
            }
            let owned = Self(job);
            let mut limits: Extended = unsafe { std::mem::zeroed() };
            limits.basic.flags = 0x2000;
            if unsafe {
                SetInformationJobObject(
                    job,
                    9,
                    (&limits as *const Extended).cast(),
                    std::mem::size_of::<Extended>() as u32,
                )
            } == 0
                || unsafe { AssignProcessToJobObject(job, raw.cast()) } == 0
            {
                return Err("Cannot assign plugin to lifecycle job".into());
            }
            Ok(owned)
        }
        pub fn terminate(&self) {
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }
    impl Drop for OwnedJob {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, PluginRuntimeConfig, PluginExecutionInput) {
        let temp = tempfile::tempdir().unwrap();
        let entry = temp.path().join("plugin.mjs");
        std::fs::write(&entry,r#"import {writeSync} from 'node:fs'; import {spawn} from 'node:child_process';
export function activate(ctx){let denied=false;ctx.storage.get('activation').catch(()=>denied=true);
ctx.handle('action:echo',async(input,i)=>({data:{input,denied,caller:i.caller,operationId:i.operationId,value:await ctx.storage.get('key')}}));
ctx.handle('action:spin',async()=>{await ctx.storage.get('entered');while(true){}});
ctx.handle('action:forget',()=>{void ctx.storage.get('forgotten');return {message:'done'};});
ctx.handle('action:descendant',async()=>{const child=spawn(process.execPath,['-e','setInterval(()=>{},1000)'],{stdio:'ignore'});await ctx.storage.set('descendant',child.pid);while(true){}});
ctx.handle('action:throw',()=>{throw new Error('Author failure')});ctx.handle('action:crash',()=>process.exit(42));
ctx.handle('action:malformed',()=>{writeSync(1,'invalid\n');return {}});}"#).unwrap();
        let config = PluginRuntimeConfig {
            executable: Some(
                which::which("bun").expect("Bun is required for isolated plugin runtime tests"),
            ),
            args: vec![PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../sidecar/plugin-host/main.mjs")
                .to_string_lossy()
                .into()],
            startup_timeout: Duration::from_secs(5),
            invocation_timeout: Duration::from_secs(5),
            stop_grace: Duration::ZERO,
            restart_backoff: Duration::ZERO,
            max_failures: 2,
        };
        let input = PluginExecutionInput {
            installation_id: "test".into(),
            generation: "generation".into(),
            directory: temp.path().into(),
            entry: "plugin.mjs".into(),
            data_directory: temp.path().into(),
            handler: "action:echo".into(),
            operation_id: "operation".into(),
            input: json!({"hello":true}),
            caller: json!({"kind":"client"}),
            context: None,
            capabilities: vec!["storage".into()],
        };
        (temp, config, input)
    }
    fn host() -> PluginHostCallback {
        Arc::new(|call| {
            Box::pin(async move {
                assert_eq!(call.installation_id, "test");
                assert_eq!(call.operation_id, "operation");
                Ok(json!("host-value"))
            })
        })
    }
    #[tokio::test]
    async fn isolated_worker_uses_host_identity_and_retains_operation_id() {
        let (_temp, config, input) = fixture();
        let supervisor = PluginSupervisor::new(config);
        let output = supervisor
            .invoke(input, host(), RuntimeCancellation::default())
            .await
            .unwrap();
        assert_eq!(output["data"]["operationId"], "operation");
        assert_eq!(output["data"]["value"], "host-value");
        assert_eq!(output["data"]["denied"], true);
        supervisor.shutdown().await;
    }
    #[tokio::test]
    async fn failures_quarantine_only_the_affected_plugin_and_resume_resets_health() {
        let (_temp, config, mut input) = fixture();
        let supervisor = PluginSupervisor::new(config);
        input.handler = "action:crash".into();
        for _ in 0..2 {
            assert!(supervisor
                .invoke(input.clone(), host(), RuntimeCancellation::default())
                .await
                .is_err());
        }
        assert_eq!(
            supervisor
                .invoke(input.clone(), host(), RuntimeCancellation::default())
                .await
                .unwrap_err()
                .reason,
            "quarantined"
        );
        supervisor.resume("test").await;
        input.handler = "action:echo".into();
        assert!(supervisor
            .invoke(input, host(), RuntimeCancellation::default())
            .await
            .is_ok());
        supervisor.shutdown().await;
    }
    #[tokio::test]
    async fn cancellation_revokes_callback_and_stale_generation_before_effects() {
        let (_temp, config, input) = fixture();
        let supervisor = Arc::new(PluginSupervisor::new(config));
        let cancel = RuntimeCancellation::default();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let entered = Arc::new(Mutex::new(Some(entered)));
        let callback: PluginHostCallback = Arc::new(move |call| {
            let entered = entered.clone();
            Box::pin(async move {
                entered
                    .lock()
                    .unwrap()
                    .take()
                    .unwrap()
                    .send(call.cancellation.clone())
                    .unwrap();
                std::future::pending().await
            })
        });
        let task = {
            let supervisor = supervisor.clone();
            let cancel = cancel.clone();
            let input = input.clone();
            tokio::spawn(async move { supervisor.invoke(input, callback, cancel).await })
        };
        let host_signal = ready.await.unwrap();
        cancel.cancel();
        assert_eq!(task.await.unwrap().unwrap_err().reason, "cancelled");
        assert!(host_signal.is_cancelled());
        supervisor.stop("test").await;
        assert_eq!(
            supervisor
                .invoke(input, host(), RuntimeCancellation::default())
                .await
                .unwrap_err()
                .reason,
            "stale_generation"
        );
        supervisor.shutdown().await;
    }
    #[tokio::test]
    async fn handler_errors_do_not_quarantine_and_safe_mode_refuses_execution() {
        let (_temp, config, mut input) = fixture();
        let supervisor = PluginSupervisor::new(config);
        input.handler = "action:throw".into();
        for _ in 0..4 {
            assert_eq!(
                supervisor
                    .invoke(input.clone(), host(), RuntimeCancellation::default())
                    .await
                    .unwrap_err()
                    .reason,
                "operation_failed"
            );
        }
        supervisor.set_enabled(false).await;
        assert_eq!(
            supervisor
                .invoke(input, host(), RuntimeCancellation::default())
                .await
                .unwrap_err()
                .reason,
            "disabled"
        );
        supervisor.shutdown().await;
    }
    #[tokio::test]
    async fn bounded_reader_rejects_unterminated_oversized_frame() {
        let bytes = vec![b'x'; MAX_MESSAGE + 2];
        let mut reader = BufReader::new(bytes.as_slice());
        assert_eq!(
            read_message(&mut reader).await.unwrap_err().reason,
            "invalid_result"
        );
    }
    #[tokio::test]
    async fn cpu_loop_deadline_isolated_from_healthy_siblings() {
        let (_temp, mut config, mut input) = fixture();
        config.invocation_timeout = Duration::from_millis(100);
        let supervisor = PluginSupervisor::new(config);
        input.handler = "action:spin".into();
        assert_eq!(
            supervisor
                .invoke(input.clone(), host(), RuntimeCancellation::default())
                .await
                .unwrap_err()
                .reason,
            "timeout"
        );
        input.installation_id = "sibling".into();
        input.handler = "action:echo".into();
        let host: PluginHostCallback = Arc::new(|_| Box::pin(async { Ok(json!("healthy")) }));
        assert_eq!(
            supervisor
                .invoke(input, host, RuntimeCancellation::default())
                .await
                .unwrap()["data"]["value"],
            "healthy"
        );
        supervisor.shutdown().await;
    }
    #[tokio::test]
    async fn forgotten_host_promises_do_not_wedge_invocation_and_malformed_ipc_is_refused() {
        let (_temp, config, mut input) = fixture();
        let supervisor = PluginSupervisor::new(config);
        let pending_host: PluginHostCallback = Arc::new(|_| Box::pin(std::future::pending()));
        input.handler = "action:forget".into();
        assert_eq!(
            supervisor
                .invoke(input.clone(), pending_host, RuntimeCancellation::default())
                .await
                .unwrap()["message"],
            "done"
        );
        input.handler = "action:malformed".into();
        assert_eq!(
            supervisor
                .invoke(input, host(), RuntimeCancellation::default())
                .await
                .unwrap_err()
                .reason,
            "invalid_result"
        );
        supervisor.shutdown().await;
    }
    #[tokio::test]
    async fn dropping_invoke_future_reaps_worker_and_allows_next_admitted_invocation() {
        let (_temp, config, input) = fixture();
        let supervisor = Arc::new(PluginSupervisor::new(config));
        let (entered, ready) = tokio::sync::oneshot::channel();
        let entered = Arc::new(Mutex::new(Some(entered)));
        let callback: PluginHostCallback = Arc::new(move |call| {
            let entered = entered.clone();
            Box::pin(async move {
                entered
                    .lock()
                    .unwrap()
                    .take()
                    .unwrap()
                    .send(call.cancellation)
                    .unwrap();
                std::future::pending().await
            })
        });
        let task = {
            let supervisor = supervisor.clone();
            let input = input.clone();
            tokio::spawn(async move {
                supervisor
                    .invoke(input, callback, RuntimeCancellation::default())
                    .await
            })
        };
        let signal = ready.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(signal.is_cancelled());
        assert!(supervisor
            .invoke(input, host(), RuntimeCancellation::default())
            .await
            .is_ok());
        supervisor.shutdown().await;
    }
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn cancellation_kills_owned_process_group_descendant() {
        use std::os::fd::{AsRawFd, FromRawFd};
        let (_temp, config, mut input) = fixture();
        input.handler = "action:descendant".into();
        let supervisor = Arc::new(PluginSupervisor::new(config));
        let cancel = RuntimeCancellation::default();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let entered = Arc::new(Mutex::new(Some(entered)));
        let callback: PluginHostCallback = Arc::new(move |call| {
            let entered = entered.clone();
            Box::pin(async move {
                let pid = call.input["value"].as_i64().unwrap();
                let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
                assert!(fd >= 0);
                let pidfd = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
                entered.lock().unwrap().take().unwrap().send(pidfd).unwrap();
                Ok(Value::Null)
            })
        });
        let task = {
            let supervisor = supervisor.clone();
            let cancel = cancel.clone();
            tokio::spawn(async move { supervisor.invoke(input, callback, cancel).await })
        };
        let pidfd = ready.await.unwrap();
        cancel.cancel();
        assert_eq!(task.await.unwrap().unwrap_err().reason, "cancelled");
        let exited = tokio::task::spawn_blocking(move || {
            let mut poll = libc::pollfd {
                fd: pidfd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            (unsafe { libc::poll(&mut poll, 1, 1000) }) == 1
        })
        .await
        .unwrap();
        assert!(exited, "Owned descendant must exit after cancellation");
        supervisor.shutdown().await;
    }
}
