//! Dedicated bounded transport. Never reuse the trusted provider JsonRpcChild.
use super::{ErrorCode, Manifest, ProtocolError, Result};
use codemux_addon_protocol::{limits, wire::Envelope};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Child,
    sync::mpsc,
};
use tokio_util::sync::CancellationToken;
pub enum Event {
    Message(Envelope),
    Stopped(ProtocolError),
}
/// Fixed reasons the trusted host prints to stderr before exiting. Nothing else
/// from that stream is surfaced, so plugin data cannot reach diagnostics.
const HOST_STOPS: &[(&str, ErrorCode)] = &[
    ("Plugin initialization failed", ErrorCode::PluginStopped),
    ("Plugin callback failed", ErrorCode::PluginStopped),
    ("Plugin promise failed", ErrorCode::PluginStopped),
    (
        "Unhandled plugin promise rejection",
        ErrorCode::PluginStopped,
    ),
    ("Plugin CPU deadline exceeded", ErrorCode::ResourceLimit),
    ("Microtask deadline exceeded", ErrorCode::ResourceLimit),
    ("Active CPU budget exceeded", ErrorCode::ResourceLimit),
    (
        "Repeated protocol or quota violations",
        ErrorCode::ResourceLimit,
    ),
    ("UI update queue overflow", ErrorCode::ResourceLimit),
    ("Outgoing frame limit", ErrorCode::ResourceLimit),
    ("Bundle limit", ErrorCode::ResourceLimit),
    ("Serialization failed", ErrorCode::InvalidMessage),
    ("Invalid parent message", ErrorCode::InvalidMessage),
    ("Invalid initialization", ErrorCode::InvalidMessage),
    ("Invalid source frame", ErrorCode::InvalidMessage),
    ("Invalid manifest", ErrorCode::InvalidMessage),
    ("Expected initialize", ErrorCode::InvalidMessage),
    ("Expected source chunk", ErrorCode::InvalidMessage),
    ("Missing initialization", ErrorCode::InvalidMessage),
    ("Missing source chunk", ErrorCode::InvalidMessage),
    ("Incompatible protocol", ErrorCode::IncompatibleApi),
    ("Initialization deadline", ErrorCode::Timeout),
    ("Engine initialization failed", ErrorCode::PluginStopped),
    ("Context initialization failed", ErrorCode::PluginStopped),
];
fn host_stop(stderr: &[u8]) -> Option<ProtocolError> {
    String::from_utf8_lossy(stderr).lines().find_map(|line| {
        let reason = line.strip_prefix("Plugin host stopped: ")?;
        HOST_STOPS
            .iter()
            .find(|(known, _)| *known == reason)
            .map(|(known, code)| ProtocolError::new(*code, format!("Plugin host stopped: {known}")))
    })
}
/// An exiting host explains itself on stderr; wait briefly for that line.
async fn explained(
    reason: &mut tokio::task::JoinHandle<Option<ProtocolError>>,
    error: ProtocolError,
) -> ProtocolError {
    match tokio::time::timeout(Duration::from_millis(250), reason).await {
        Ok(Ok(Some(reason))) => reason,
        _ => error,
    }
}
/// At least five violations within 10 s.
pub(super) fn repeated(violations: &mut VecDeque<Instant>, now: Instant) -> bool {
    while violations
        .front()
        .is_some_and(|t| now.saturating_duration_since(*t) >= Duration::from_secs(10))
    {
        violations.pop_front();
    }
    violations.push_back(now);
    violations.len() >= 5
}
struct Outbound {
    bytes: Vec<u8>,
}
struct HostLease(CancellationToken);
impl Drop for HostLease {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
#[derive(Clone)]
pub struct Host {
    // The supervising task owns the child, not this lease. Losing the last
    // caller (including a cancelled/failed initialization) must reap the child.
    _lease: Arc<HostLease>,
    pub generation: String,
    writer: mpsc::Sender<Outbound>,
    cancel: CancellationToken,
    done: CancellationToken,
    sequence: Arc<AtomicU64>,
    progress: Arc<Mutex<HashMap<u64, Instant>>>,
}
impl Host {
    pub async fn spawn(
        executable: &Path,
        manifest: &Manifest,
        source: &str,
    ) -> Result<(Self, mpsc::Receiver<Event>)> {
        let directory = tempfile::tempdir().map_err(|_| {
            ProtocolError::new(
                ErrorCode::StorageUnavailable,
                "Cannot create private host directory",
            )
        })?;
        let mut command = tokio::process::Command::new(executable);
        command
            .env_clear()
            .current_dir(directory.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // The host closes unrelated inherited descriptors itself before it
        // starts, so this spawn keeps the fast path instead of forking the app.
        #[cfg(windows)]
        {
            if let Some(value) = std::env::var_os("SystemRoot") {
                command.env("SystemRoot", value);
            }
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn().map_err(|_| {
            ProtocolError::new(
                ErrorCode::PluginStopped,
                "Bundled plugin host is unavailable",
            )
        })?;
        let mut stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        // Drain stderr so the host never blocks on it; keep only the first 1 KiB.
        let mut reason = tokio::spawn(async move {
            let (mut kept, mut buffer) = (Vec::new(), [0; 4096]);
            while let Ok(count @ 1..) = stderr.read(&mut buffer).await {
                let room = 1024usize.saturating_sub(kept.len());
                kept.extend_from_slice(&buffer[..count.min(room)]);
            }
            host_stop(&kept)
        });
        let (writer, mut writes) = mpsc::channel::<Outbound>(2);
        let (events, received) = mpsc::channel(2);
        let cancel = CancellationToken::new();
        let done = CancellationToken::new();
        let generation = uuid::Uuid::new_v4().to_string();
        let progress = Arc::new(Mutex::new(HashMap::new()));
        let host = Self {
            _lease: Arc::new(HostLease(cancel.clone())),
            generation: generation.clone(),
            writer,
            cancel: cancel.clone(),
            done: done.clone(),
            sequence: Arc::new(AtomicU64::new(1)),
            progress: progress.clone(),
        };
        let writing_cancel = cancel.clone();
        // A broken input pipe usually means the host is exiting. Stop writing
        // but let the reader observe the exit and its reason; later writes fail
        // and pending calls still reach the watchdog.
        let write_task = tokio::spawn(async move {
            loop {
                tokio::select! {_ = writing_cancel.cancelled()=>break,write=writes.recv()=>{let Some(write)=write else{break};if stdin.write_all(&write.bytes).await.is_err(){break}}}
            }
        });
        let replies = host.writer.clone();
        tokio::spawn(async move {
            let _private_directory = directory;
            let mut reader = BufReader::new(stdout);
            let mut frame = Vec::new();
            let mut requests = limits::RateLimit::default();
            let mut violations = VecDeque::new();
            let mut interval = tokio::time::interval(Duration::from_millis(500));
            let mut system = sysinfo::System::new();
            let pid = child.id().map(sysinfo::Pid::from_u32);
            let result:Result<()>=async{loop{
    let next_deadline = progress.lock().unwrap().values().min().map(|time| *time + Duration::from_millis(1500));
    let watchdog = async { match next_deadline { Some(deadline) => tokio::time::sleep_until(deadline.into()).await, None => std::future::pending::<()>().await } };
    tokio::select!{
     biased;
     _ = cancel.cancelled()=>return Ok(()),
     _ = watchdog=>return Err(ProtocolError::new(ErrorCode::Timeout,"Plugin host stopped responding")),
     _ = interval.tick()=>{
      if child.try_wait().map_err(|_|ProtocolError::new(ErrorCode::PluginStopped,"Plugin host exited"))?.is_some(){return Err(explained(&mut reason,ProtocolError::new(ErrorCode::PluginStopped,"Plugin host exited")).await)}
      if let Some(pid)=pid{system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::Some(&[pid]),true,sysinfo::ProcessRefreshKind::nothing().with_memory());if system.process(pid).is_some_and(|p|p.memory()>192*1024*1024){return Err(ProtocolError::new(ErrorCode::ResourceLimit,"Plugin host memory limit"))}}
     },
     result=read_frame(&mut reader,&mut frame)=>{
      if let Err(error)=result{return Err(if error.data.code==ErrorCode::PluginStopped{explained(&mut reason,error).await}else{error})}
      // The trusted host validates every plugin frame, so a malformed one here
      // means the host itself is compromised or broken: stop at once.
      let message=Envelope::parse(&frame,Some(&generation),true)?;frame.clear();
      if message.method.is_none() { if let Some(error)=message.error { return Err(error) } continue; }
      // The host enforces the exact quota at the source. This backstop bounds
      // only a broken host: it allows twice that to absorb arrival bunching,
      // answers excess requests, and stops only on repeated violations.
      if message.method.as_deref()==Some("host.request")&&!requests.accept(Instant::now(),40,200){
       let quota=ProtocolError::new(ErrorCode::ResourceLimit,"Plugin request quota exceeded");
       if repeated(&mut violations,Instant::now()){return Err(quota)}
       // Violations bound these replies to four per 10 s, so each may wait for
       // the input queue in its own task while the reader moves on.
       if let Some(id)=message.id{let mut bytes=serde_json::to_vec(&Envelope::response(&generation,id,Err(quota))).unwrap();bytes.push(b'\n');let (replies,cancel)=(replies.clone(),cancel.clone());tokio::spawn(async move{tokio::select!{_=cancel.cancelled()=>{},_=tokio::time::timeout(Duration::from_millis(1500),replies.send(Outbound{bytes}))=>{}}});}
       continue;
      }
      if message.method.as_deref()==Some("ready")&&message.params.as_ref().is_some_and(|p|p["phase"]=="yielded"){
       if let Some(id)=message.params.as_ref().and_then(|p|p["requestId"].as_u64()){progress.lock().unwrap().remove(&id);}
      }
      // Bounded backpressure; never discard a patch and continue a corrupt tree.
      tokio::select!{_ = cancel.cancelled()=>return Ok(()),result=tokio::time::timeout(Duration::from_millis(1500),events.send(Event::Message(message)))=>{result.map_err(|_|ProtocolError::new(ErrorCode::ResourceLimit,"Plugin event queue overflow"))?.map_err(|_|ProtocolError::new(ErrorCode::PluginStopped,"Plugin receiver closed"))?;}}
     }
    }
   }}.await;
            cancel.cancel();
            write_task.abort();
            let _ = write_task.await;
            // Revoke broker authority even if the kernel delays termination.
            // Closing a full event queue also makes the manager stop after it
            // drains the already-bounded messages; it cannot wait on reaping.
            if let Err(error) = result {
                let _ = events.try_send(Event::Stopped(error));
            }
            drop(events);
            reap(&mut child).await;
            done.cancel();
        });
        if source.len() > limits::BUNDLE {
            host.stop().await;
            return Err(ProtocolError::invalid("Bundle too large"));
        }
        // JSON escaping can expand one byte to six. Keep serialized chunks <1 MiB.
        let mut remaining = source;
        let mut first = true;
        loop {
            let mut end = remaining.len().min(128 * 1024);
            while !remaining.is_char_boundary(end) {
                end -= 1
            }
            let (chunk, rest) = remaining.split_at(end);
            let mut params = json!({"source":chunk,"final":rest.is_empty()});
            if first {
                params["protocolVersion"] = json!(1);
                params["manifest"] = serde_json::to_value(manifest).unwrap();
            }
            host.send("initialize", params).await?;
            first = false;
            remaining = rest;
            if remaining.is_empty() {
                break;
            }
        }
        Ok((host, received))
    }
    pub async fn send(&self, method: &str, params: Value) -> Result<u64> {
        let id = self.sequence.fetch_add(1, Ordering::Relaxed);
        if method != "initialize" {
            let mut progress = self.progress.lock().unwrap();
            if progress.len() >= 16 {
                return Err(ProtocolError::new(
                    ErrorCode::ResourceLimit,
                    "Too many host calls",
                ));
            }
            progress.insert(id, Instant::now());
        }
        self.write(json!({"jsonrpc":"2.0","generation":self.generation,"id":id,"method":method,"params":params})).await?;
        Ok(id)
    }
    pub async fn respond(&self, id: u64, result: Result<Value>) -> Result<()> {
        self.write(serde_json::to_value(Envelope::response(&self.generation, id, result)).unwrap())
            .await
    }
    async fn write(&self, value: Value) -> Result<()> {
        if self.cancel.is_cancelled() {
            return Err(ProtocolError::new(
                ErrorCode::PluginStopped,
                "Plugin is stopped",
            ));
        }
        let mut bytes = serde_json::to_vec(&value)
            .map_err(|_| ProtocolError::invalid("Cannot encode host message"))?;
        if bytes.len() > limits::FRAME {
            return Err(ProtocolError::new(
                ErrorCode::ResourceLimit,
                "Host message exceeds limit",
            ));
        }
        bytes.push(b'\n');
        tokio::select! {_ = self.cancel.cancelled()=>Err(ProtocolError::new(ErrorCode::PluginStopped,"Plugin is stopped")),result=tokio::time::timeout(Duration::from_millis(1500),self.writer.send(Outbound{bytes}))=>result.map_err(|_|ProtocolError::new(ErrorCode::Timeout,"Plugin input queue timed out"))?.map_err(|_|ProtocolError::new(ErrorCode::PluginStopped,"Plugin input closed"))}
    }
    pub async fn stop(&self) -> bool {
        let finished = tokio::time::timeout(Duration::from_millis(500), async {
            let _ = self.send("deactivate", json!({})).await;
            self.done.cancelled().await;
        })
        .await;
        if finished.is_ok() {
            return true;
        }
        self.cancel.cancel();
        tokio::time::timeout(Duration::from_millis(1500), self.done.cancelled())
            .await
            .is_ok()
    }
    pub async fn reaped(&self) {
        self.done.cancelled().await;
    }
    /// Parent calls the child has accepted but not yet yielded.
    pub fn pending_calls(&self) -> bool {
        !self.progress.lock().unwrap().is_empty()
    }
    pub fn revoke(&self) {
        self.cancel.cancel();
    }
}
async fn reap(child: &mut Child) {
    let _ = child.kill().await;
    let _ = child.wait().await;
}
async fn read_frame(
    reader: &mut BufReader<tokio::process::ChildStdout>,
    frame: &mut Vec<u8>,
) -> Result<()> {
    loop {
        let bytes = reader
            .fill_buf()
            .await
            .map_err(|_| ProtocolError::new(ErrorCode::PluginStopped, "Plugin pipe closed"))?;
        if bytes.is_empty() {
            return Err(ProtocolError::new(
                ErrorCode::PluginStopped,
                "Plugin pipe closed",
            ));
        }
        let end = bytes.iter().position(|b| *b == b'\n');
        let count = end.unwrap_or(bytes.len());
        if frame.len() + count > limits::FRAME {
            return Err(ProtocolError::new(
                ErrorCode::ResourceLimit,
                "Plugin frame exceeds 1 MiB",
            ));
        }
        frame.extend_from_slice(&bytes[..count]);
        reader.consume(count + usize::from(end.is_some()));
        if end.is_some() {
            return Ok(());
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn unresponsive(root: &Path) -> std::path::PathBuf {
        script(root, "unresponsive-host", "exec /bin/sleep 60")
    }
    /// A fake host. The environment is cleared, so commands use absolute paths.
    fn script(root: &Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = root.join(name);
        // exec replaces the shell: the supervisor owns the only process.
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
    fn manifest() -> Manifest {
        Manifest::parse(
            include_bytes!("../../addon-protocol/fixtures/hello.json"),
            None,
        )
        .unwrap()
    }
    /// The stop reason, or None when a full event queue dropped it (the
    /// manager then reports the exit), and the messages forwarded before it.
    async fn stopped(events: &mut mpsc::Receiver<Event>) -> (Option<ProtocolError>, usize) {
        let mut messages = 0;
        loop {
            match tokio::time::timeout(Duration::from_secs(3), events.recv())
                .await
                .expect("fault must stop the generation")
            {
                Some(Event::Message(_)) => messages += 1,
                Some(Event::Stopped(error)) => return (Some(error), messages),
                None => return (None, messages),
            }
        }
    }
    // Reads the generation from the first initialize frame, floods requests
    // between pauses in which it reads nothing else, then records replies
    // while keeping its output pipe open on fd 3.
    const REQUESTS: &str = "IFS= read -r line\n\
        generation=$(printf '%s' \"$line\" | /bin/sed 's/.*\"generation\":\"\\([^\"]*\\)\".*/\\1/')\n\
        /bin/sleep PAUSE\n\
        i=1; while [ $i -le COUNT ]; do printf '{\"jsonrpc\":\"2.0\",\"generation\":\"%s\",\"id\":%d,\"method\":\"host.request\",\"params\":{\"operation\":\"settings.get\",\"params\":{}}}\\n' \"$generation\" $i; i=$((i+1)); done\n\
        /bin/sleep PAUSE\n\
        exec 3>&1\nexec /bin/cat > OUT";
    #[tokio::test]
    async fn oversized_and_malformed_child_frames_stop_the_generation() {
        let root = tempfile::tempdir().unwrap();
        for (body, code, message) in [
            (
                "/bin/head -c 1100000 /dev/zero | /bin/tr '\\0' x\nexec /bin/sleep 60",
                ErrorCode::ResourceLimit,
                "Plugin frame exceeds 1 MiB",
            ),
            (
                "printf 'not json\\n'\nexec /bin/sleep 60",
                ErrorCode::InvalidMessage,
                "Invalid JSON",
            ),
        ] {
            let executable = script(root.path(), "fake-host", body);
            let (_host, mut events) = Host::spawn(&executable, &manifest(), "").await.unwrap();
            let error = stopped(&mut events).await.0.expect("stop reason");
            assert_eq!((error.data.code, error.message.as_str()), (code, message));
        }
    }
    #[tokio::test]
    async fn parent_request_quota_answers_excess_and_stops_repeated_violations() {
        let root = tempfile::tempdir().unwrap();
        let replies = root.path().join("replies");
        let body = REQUESTS
            .replace("COUNT", "42")
            .replace("PAUSE", "0.5")
            .replace("OUT", replies.to_str().unwrap());
        let executable = script(root.path(), "flooding-host", &body);
        let (host, mut events) = Host::spawn(&executable, &manifest(), "").await.unwrap();
        // While the host reads nothing, fill its pipe and the input queue so
        // the quota replies below cannot be queued at once.
        for id in 1000..1003 {
            host.respond(id, Ok(json!("x".repeat(600_000))))
                .await
                .unwrap();
        }
        // The backstop allows twice the host's own quota before answering.
        for _ in 0..40 {
            let event = tokio::time::timeout(Duration::from_secs(2), events.recv()).await;
            assert!(matches!(event, Ok(Some(Event::Message(_)))));
        }
        // Two excess requests are answered with a bounded error, not fatal.
        assert!(
            tokio::time::timeout(Duration::from_millis(500), events.recv())
                .await
                .is_err()
        );
        let until = Instant::now() + Duration::from_secs(3);
        let rejected = loop {
            let written = std::fs::read_to_string(&replies).unwrap_or_default();
            let rejected: Vec<Value> = written
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .filter(|reply: &Value| reply["error"]["data"]["code"] == "RESOURCE_LIMIT")
                .map(|reply| reply["id"].clone())
                .collect();
            if rejected.len() >= 2 || Instant::now() >= until {
                break rejected;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert_eq!(rejected, [json!(41), json!(42)]);
        assert!(host.stop().await);
        let body = REQUESTS
            .replace("COUNT", "45")
            .replace("PAUSE", "0")
            .replace("OUT", "/dev/null");
        let executable = script(root.path(), "flooding-host", &body);
        let (_host, mut events) = Host::spawn(&executable, &manifest(), "").await.unwrap();
        let (error, messages) = stopped(&mut events).await;
        assert_eq!(messages, 40);
        assert!(error.is_none_or(|e| e.message == "Plugin request quota exceeded"));
    }
    #[tokio::test]
    async fn child_memory_above_the_limit_stops_it() {
        let root = tempfile::tempdir().unwrap();
        // dd keeps refilling one touched 220 MiB block until it is stopped. It
        // reopens of= on stdout, so fd 3 keeps the output pipe open.
        let executable = script(
            root.path(),
            "growing-host",
            "exec 3>&1\nexec /bin/dd if=/dev/zero of=/dev/null bs=220M count=1000 status=none",
        );
        let started = Instant::now();
        let (_host, mut events) = Host::spawn(&executable, &manifest(), "").await.unwrap();
        let error = stopped(&mut events).await.0.expect("stop reason");
        assert_eq!(error.message, "Plugin host memory limit");
        // Sampled every 500 ms and contained within the 2 s fault deadline.
        assert!(started.elapsed() < Duration::from_secs(2));
    }
    #[tokio::test]
    async fn only_known_host_stop_reasons_are_reported() {
        let root = tempfile::tempdir().unwrap();
        for (line, known) in [
            (
                "Plugin host stopped: Unhandled plugin promise rejection",
                true,
            ),
            ("Plugin host stopped: private token 1234", false),
            ("private plugin output", false),
        ] {
            let executable = script(
                root.path(),
                "exiting-host",
                &format!("IFS= read -r line\nprintf '%s\\n' '{line}' >&2\nexit 1"),
            );
            let (_host, mut events) = Host::spawn(&executable, &manifest(), "").await.unwrap();
            let error = stopped(&mut events).await.0.expect("stop reason");
            if known {
                assert_eq!(error.message, line);
            } else {
                // Either exit observation is generic; stderr text never leaks.
                assert!(
                    ["Plugin pipe closed", "Plugin host exited"].contains(&error.message.as_str())
                );
            }
        }
    }
    #[tokio::test]
    async fn unresponsive_child_is_reaped_inside_the_fault_and_shutdown_deadlines() {
        let root = tempfile::tempdir().unwrap();
        let executable = unresponsive(root.path());
        let manifest = Manifest::parse(
            include_bytes!("../../addon-protocol/fixtures/hello.json"),
            None,
        )
        .unwrap();
        let (host, mut events) = Host::spawn(&executable, &manifest, "").await.unwrap();
        let start = Instant::now();
        host.send("activate", json!({})).await.unwrap();
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("fault must be contained within 2 s");
        assert!(matches!(event, Some(Event::Stopped(_))));
        host.reaped().await;
        assert!(start.elapsed() < Duration::from_secs(2));
        let (host, _events) = Host::spawn(&executable, &manifest, "").await.unwrap();
        let start = Instant::now();
        assert!(host.stop().await);
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[tokio::test]
    async fn dropped_initialization_cancels_supervision_and_reaps_the_owned_child() {
        let root = tempfile::tempdir().unwrap();
        let executable = unresponsive(root.path());
        let manifest = Manifest::parse(
            include_bytes!("../../addon-protocol/fixtures/hello.json"),
            None,
        )
        .unwrap();
        let (host, _events) = Host::spawn(&executable, &manifest, "").await.unwrap();
        let reaped = host.done.clone();
        drop(host);
        tokio::time::timeout(Duration::from_secs(2), reaped.cancelled())
            .await
            .expect("last host handle must not leak a process");
    }
}
