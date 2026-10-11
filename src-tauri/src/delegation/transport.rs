use serde_json::Value;
use std::{process::Stdio, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
#[derive(Debug, Clone)]
pub enum Operation {
    Capabilities,
    Launch,
    Read { id: String, cursor: i64 },
    Cancel { id: String },
    Respond { id: String },
}
fn checked_id(id: &str) -> Result<&str, String> {
    let parsed = uuid::Uuid::parse_str(id).map_err(|_| "invalid remote task UUID")?;
    if parsed.to_string() != id {
        return Err("remote task UUID must be canonical".into());
    }
    Ok(id)
}
pub fn remote_command(op: &Operation) -> Result<String, String> {
    Ok(match op {
        Operation::Capabilities => "codemux-remote task capabilities".into(),
        Operation::Launch => "codemux-remote task launch".into(),
        Operation::Read { id, cursor } => {
            if *cursor < 0 {
                return Err("negative cursor".into());
            }
            format!(
                "codemux-remote task read --id {} --after {cursor} --wait-ms 15000",
                checked_id(id)?
            )
        }
        Operation::Cancel { id } => format!("codemux-remote task cancel --id {}", checked_id(id)?),
        Operation::Respond { id } => {
            format!("codemux-remote task respond --id {}", checked_id(id)?)
        }
    })
}
pub fn validate_target(target: &str) -> Result<(), String> {
    if target.is_empty()
        || target.len() > 255
        || target.starts_with('-')
        || !target
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@._-:[]".contains(&b))
    {
        return Err("Configured SSH target must be a host alias or user@host, without options or shell syntax".into());
    }
    Ok(())
}
#[derive(Debug, Clone)]
pub enum TransportError {
    BeforeSend(String),
    Unknown(String),
}
impl std::fmt::Display for TransportError {
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {
        match self {Self::BeforeSend(message)=>write!(f,"definitely before send: {message}"),Self::Unknown(message)=>write!(f,"unconfirmed remote outcome: {message}")}
    }
}
#[async_trait::async_trait]
pub trait TaskTransport: Send + Sync {
    async fn call(
        &self,
        target: &str,
        operation: Operation,
        input: Option<Value>,
    ) -> Result<Value, String>;
    /// Fail closed unless the transport fences its actual dispatch owner.
    async fn call_guarded(&self,_target:&str,_operation:Operation,_input:Option<Value>,_guard:std::sync::Arc<dyn crate::json_rpc_child::dispatch::DispatchGuard>)->Result<Value,TransportError> {
        Err(TransportError::BeforeSend("Transport has no guarded dispatch owner".into()))
    }
    async fn call_outcome(&self,target:&str,operation:Operation,input:Option<Value>)->Result<Value,TransportError> {
        self.call(target,operation,input).await.map_err(TransportError::Unknown)
    }
}
pub struct SshTransport;
#[cfg(test)]
pub(crate) static TEST_TRANSPORT_SPAWNED:std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<std::path::PathBuf,u32>>>=std::sync::LazyLock::new(Default::default);
#[cfg(test)]
pub(crate) static TEST_TRANSPORT_WRITES:std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<std::path::PathBuf,std::sync::Arc<crate::json_rpc_child::TestWriteGate>>>>=std::sync::LazyLock::new(Default::default);
#[async_trait::async_trait]
impl TaskTransport for SshTransport {
    async fn call(&self,target:&str,operation:Operation,input:Option<Value>)->Result<Value,String> {
        self.call_outcome(target,operation,input).await.map_err(|e|e.to_string())
    }
    async fn call_outcome(
        &self,
        target: &str,
        operation: Operation,
        input: Option<Value>,
    ) -> Result<Value, TransportError> {
        validate_target(target).map_err(TransportError::BeforeSend)?;
        let remote = remote_command(&operation).map_err(TransportError::BeforeSend)?;
        let bytes = input
            .map(|v| serde_json::to_vec(&v))
            .transpose()
            .map_err(|e| TransportError::BeforeSend(e.to_string()))?
            .unwrap_or_default();
        if bytes.len() > 65536 {
            return Err(TransportError::BeforeSend("remote JSON stdin exceeds 64 KiB".into()));
        }
        let mut command = tokio::process::Command::new("ssh");
        command.args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "ServerAliveInterval=5",
            "-o",
            "ServerAliveCountMax=2",
            "--",
            target,
            &remote,
        ]);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        dispatch_command(command,bytes,None).await
    }
    async fn call_guarded(&self,target:&str,operation:Operation,input:Option<Value>,guard:std::sync::Arc<dyn crate::json_rpc_child::dispatch::DispatchGuard>)->Result<Value,TransportError> {
        validate_target(target).map_err(TransportError::BeforeSend)?;
        let remote=remote_command(&operation).map_err(TransportError::BeforeSend)?;
        let bytes=input.map(|v|serde_json::to_vec(&v)).transpose().map_err(|e|TransportError::BeforeSend(e.to_string()))?.unwrap_or_default();
        if bytes.len()>65536 {return Err(TransportError::BeforeSend("remote JSON stdin exceeds 64 KiB".into()));}
        let mut command=tokio::process::Command::new("ssh");
        command.args(["-T","-o","BatchMode=yes","-o","StrictHostKeyChecking=yes","-o","ConnectTimeout=10","-o","ServerAliveInterval=5","-o","ServerAliveCountMax=2","--",target,&remote]);
        dispatch_command(command,bytes,Some(guard)).await
    }
}
/// Actual production dispatch owner, shared with explicit offline CLI fixtures.
/// Leases exist only inside synchronous spawn/poll closures, never across await.
pub(crate) async fn dispatch_command(mut command:tokio::process::Command,bytes:Vec<u8>,guard:Option<std::sync::Arc<dyn crate::json_rpc_child::dispatch::DispatchGuard>>)->Result<Value,TransportError> {
    use std::{pin::Pin,task::Poll};
    use tokio::io::AsyncWrite;
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(test)]
    let gate=TEST_TRANSPORT_WRITES.lock().unwrap().get(std::path::Path::new(command.as_std().get_program())).cloned();
    let mut spawned=None;
    let mut spawn=|| {spawned=Some(command.spawn());Poll::Ready(Ok(0))};
    let spawned_poll=if let Some(guard)=&guard {guard.poll_first(&mut spawn).map_err(TransportError::BeforeSend)?} else {spawn()};
    if !matches!(spawned_poll,Poll::Ready(Ok(_))) {return Err(TransportError::BeforeSend("Dispatch guard refused synchronous spawn".into()));}
    let mut child=spawned.ok_or_else(||TransportError::BeforeSend("Dispatch guard did not invoke spawn".into()))?.map_err(|e|TransportError::BeforeSend(format!("Start task transport: {e}")))?;
    #[cfg(test)]
    if gate.is_some() {TEST_TRANSPORT_SPAWNED.lock().unwrap().insert(std::path::PathBuf::from(command.as_std().get_program()),child.id().unwrap());}
    let mut stdin=child.stdin.take().ok_or_else(||TransportError::Unknown("Task stdin unavailable".into()))?;
    let stdout=child.stdout.take().ok_or_else(||TransportError::Unknown("Task stdout unavailable".into()))?;
    let stderr=child.stderr.take().ok_or_else(||TransportError::Unknown("Task stderr unavailable".into()))?;
    let exchange=async {
        let writing=async {
            let mut offset=0;
            while offset<bytes.len() {
                let n=futures_util::future::poll_fn(|cx| {
                    if let Some(guard)=&guard {guard.register(cx.waker());}
                    let mut write=|| {
                        #[cfg(test)]
                        if offset==0 {
                            if let Some(gate)=&gate {
                                gate.waker.register(cx.waker());
                                if !gate.released.load(std::sync::atomic::Ordering::SeqCst) {gate.entered.notify_one();return Poll::Pending;}
                            }
                        }
                        Pin::new(&mut stdin).poll_write(cx,&bytes[offset..])
                    };
                    if offset==0 {
                        if let Some(guard)=&guard {return match guard.poll_first(&mut write) {Ok(p)=>p,Err(e)=>Poll::Ready(Err(std::io::Error::other(e)))};}
                    }
                    write()
                }).await.map_err(|e|e.to_string())?;
                if n==0 {return Err("Task stdin accepted zero bytes".into());}
                // A positive write admits this frame; finish it, never start
                // a new one under stale authority. This child owns one request.
                offset+=n;
            }
            stdin.shutdown().await.map_err(|e|e.to_string())?;
            drop(stdin); // ChildStdin shutdown alone does not close the pipe.
            Ok::<(),String>(())
        };
        let ((),out,err,status)=tokio::try_join!(writing,bounded(stdout,2*1024*1024),bounded(stderr,16384),async {child.wait().await.map_err(|e|e.to_string())})?;
        if !status.success() {return Err(format!("Remote task request failed ({status}): {}",String::from_utf8_lossy(&err).trim()));}
        serde_json::from_slice(&out).map_err(|e|format!("Invalid remote task JSON: {e}"))
    };
    let outcome=tokio::time::timeout(Duration::from_secs(50),exchange).await.map_err(|_|"Task request timed out; remote execution may continue".to_string()).and_then(|r|r);
    if outcome.is_err() {let _=child.kill().await;let _=child.wait().await;}
    // Once spawn succeeded, all failures remain Unknown, even zero-byte guard
    // refusal: the remote command may already have begun executing.
    outcome.map_err(TransportError::Unknown)
}

async fn bounded(reader: impl AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err("SSH output exceeded its bounded transport budget".into());
    }
    Ok(bytes)
}
