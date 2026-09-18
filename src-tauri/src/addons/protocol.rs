//! Dedicated bounded transport. Never reuse the trusted provider JsonRpcChild.
use super::{ErrorCode, Manifest, ProtocolError, Result};
use codemux_addon_protocol::{limits, wire::Envelope};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::Path,
    process::Stdio,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Child,
    sync::{mpsc, Mutex},
};
use tokio_util::sync::CancellationToken;
pub enum Event {
    Message(Envelope),
    Stopped(ProtocolError),
}
struct Outbound {
    bytes: Vec<u8>,
}
#[derive(Clone)]
pub struct Host {
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
            .stderr(Stdio::null())
            .kill_on_drop(true);
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
        let (writer, mut writes) = mpsc::channel::<Outbound>(2);
        let (events, received) = mpsc::channel(2);
        let cancel = CancellationToken::new();
        let done = CancellationToken::new();
        let generation = uuid::Uuid::new_v4().to_string();
        let progress = Arc::new(Mutex::new(HashMap::new()));
        let host = Self {
            generation: generation.clone(),
            writer,
            cancel: cancel.clone(),
            done: done.clone(),
            sequence: Arc::new(AtomicU64::new(1)),
            progress: progress.clone(),
        };
        let writing_cancel = cancel.clone();
        let write_task = tokio::spawn(async move {
            loop {
                tokio::select! {_ = writing_cancel.cancelled()=>break,write=writes.recv()=>{let Some(write)=write else{break};if stdin.write_all(&write.bytes).await.is_err(){writing_cancel.cancel();break}}}
            }
        });
        tokio::spawn(async move {
            let _private_directory = directory;
            let mut reader = BufReader::new(stdout);
            let mut frame = Vec::new();
            let mut violations = limits::RateLimit::default();
            let mut interval = tokio::time::interval(Duration::from_millis(500));
            let mut system = sysinfo::System::new();
            let pid = child.id().map(sysinfo::Pid::from_u32);
            let result:Result<()>=async{loop{
    let next_deadline = progress.lock().await.values().min().map(|time| *time + Duration::from_secs(2));
    let watchdog = async { match next_deadline { Some(deadline) => tokio::time::sleep_until(deadline.into()).await, None => std::future::pending::<()>().await } };
    tokio::select!{
     biased;
     _ = cancel.cancelled()=>return Ok(()),
     _ = watchdog=>return Err(ProtocolError::new(ErrorCode::Timeout,"Plugin host stopped responding")),
     _ = interval.tick()=>{
      if child.try_wait().map_err(|_|ProtocolError::new(ErrorCode::PluginStopped,"Plugin host exited"))?.is_some(){return Err(ProtocolError::new(ErrorCode::PluginStopped,"Plugin host exited"))}
      if let Some(pid)=pid{system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::Some(&[pid]),true,sysinfo::ProcessRefreshKind::nothing().with_memory());if system.process(pid).is_some_and(|p|p.memory()>192*1024*1024){return Err(ProtocolError::new(ErrorCode::ResourceLimit,"Plugin host memory limit"))}}
     },
     result=read_frame(&mut reader,&mut frame)=>{
      result?;
      let message=Envelope::parse(&frame,Some(&generation),true)?;frame.clear();
      if message.method.is_none() { if let Some(error)=message.error { return Err(error) } continue; }
      if message.method.as_deref()==Some("host.request")&&!violations.accept(Instant::now(),20,100){return Err(ProtocolError::new(ErrorCode::ResourceLimit,"Plugin request quota exceeded"))}
      if message.method.as_deref()==Some("ready")&&message.params.as_ref().is_some_and(|p|p["phase"]=="yielded"){
       if let Some(id)=message.params.as_ref().and_then(|p|p["requestId"].as_u64()){progress.lock().await.remove(&id);}
      }
      // Bounded backpressure; never discard a patch and continue a corrupt tree.
      tokio::select!{_ = cancel.cancelled()=>return Ok(()),result=tokio::time::timeout(Duration::from_secs(2),events.send(Event::Message(message)))=>{result.map_err(|_|ProtocolError::new(ErrorCode::ResourceLimit,"Plugin event queue overflow"))?.map_err(|_|ProtocolError::new(ErrorCode::PluginStopped,"Plugin receiver closed"))?;}}
     }
    }
   }}.await;
            cancel.cancel();
            write_task.abort();
            let _ = write_task.await;
            reap(&mut child).await;
            if let Err(error) = result {
                let _ = events.try_send(Event::Stopped(error));
            }
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
            let mut progress = self.progress.lock().await;
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
        tokio::select! {_ = self.cancel.cancelled()=>Err(ProtocolError::new(ErrorCode::PluginStopped,"Plugin is stopped")),result=tokio::time::timeout(Duration::from_secs(2),self.writer.send(Outbound{bytes}))=>result.map_err(|_|ProtocolError::new(ErrorCode::Timeout,"Plugin input queue timed out"))?.map_err(|_|ProtocolError::new(ErrorCode::PluginStopped,"Plugin input closed"))}
    }
    pub async fn stop(&self) {
        let finished = tokio::time::timeout(Duration::from_millis(500), async {
            let _ = self.send("deactivate", json!({})).await;
            self.done.cancelled().await;
        })
        .await;
        if finished.is_ok() {
            return;
        }
        self.cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_millis(1500), self.done.cancelled()).await;
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
