use super::RunPermit;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{Child, Command},
    sync::watch,
};

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
const DEFAULT_OUTPUT: usize = 1024 * 1024;
const MAX_OUTPUT: usize = 8 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunInput {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, Option<String>>,
    pub timeout_ms: Option<u64>,
    pub max_output_bytes: Option<usize>,
    pub workspace_id: Option<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunResult {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub truncated: bool,
    pub cancelled: bool,
}

fn validate_command(command: &str) -> Result<(), String> {
    if command.is_empty() || command.len() > 4096 || command.contains('\0') {
        Err("Invalid command".into())
    } else {
        Ok(())
    }
}
fn path_value(env: &HashMap<String, Option<String>>, key: &str) -> Option<std::ffi::OsString> {
    let value = env.iter().find(|(name, _)| {
        if cfg!(windows) {
            name.eq_ignore_ascii_case(key)
        } else {
            name.as_str() == key
        }
    });
    match value {
        Some((_, value)) => value.as_ref().map(Into::into),
        None if key == "PATH" => Some(crate::execution::sanitized_child_path().into()),
        None => std::env::var_os(key),
    }
}

#[cfg(any(windows, test))]
fn windows_candidate(attributes: u32) -> bool {
    // App Execution Aliases are zero-byte reparse points. CreateProcess owns
    // their interpretation; only directory candidates are excluded here.
    attributes & 0x10 == 0 // FILE_ATTRIBUTE_DIRECTORY
}

fn resolve(
    command: &str,
    paths: impl IntoIterator<Item = PathBuf>,
    cwd: &Path,
    extensions: &str,
    windows: bool,
) -> Option<PathBuf> {
    let command = Path::new(command);
    let bases: Vec<PathBuf> = if command.is_absolute() {
        vec![command.into()]
    } else if command.components().count() > 1 {
        vec![cwd.join(command)]
    } else {
        paths
            .into_iter()
            .map(|dir| {
                if dir.is_absolute() {
                    dir.join(command)
                } else {
                    cwd.join(dir).join(command)
                }
            })
            .collect()
    };
    for base in bases {
        let mut candidates = vec![base.clone()];
        if windows && base.extension().is_none() {
            candidates.extend(
                extensions
                    .split(';')
                    .filter(|ext| {
                        ext.starts_with('.')
                            && ext.len() <= 16
                            && ext[1..].bytes().all(|b| b.is_ascii_alphanumeric())
                    })
                    .map(|ext| PathBuf::from(format!("{}{ext}", base.to_string_lossy()))),
            );
        }
        for path in candidates {
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if windows {
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if !windows_candidate(meta.file_attributes()) {
                        continue;
                    }
                }
                // This branch permits resolution-policy tests on Unix. Actual
                // Windows classification uses attributes, including reparse points.
                #[cfg(not(windows))]
                if !meta.is_file() {
                    continue;
                }
            } else {
                let Ok(meta) = std::fs::metadata(&path) else {
                    continue;
                };
                if !meta.is_file() {
                    continue;
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if meta.permissions().mode() & 0o111 == 0 {
                        continue;
                    }
                }
            }
            return Some(path);
        }
    }
    None
}
fn executable(command: &str, cwd: &Path, env: &HashMap<String, Option<String>>) -> Option<PathBuf> {
    let path = path_value(env, "PATH").unwrap_or_default();
    let extensions = path_value(env, "PATHEXT").unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
    resolve(
        command,
        std::env::split_paths(&path),
        cwd,
        &extensions.to_string_lossy(),
        cfg!(windows),
    )
}

fn validate_program(program: &Path, windows: bool) -> Result<(), String> {
    if windows
        && program
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"))
    {
        return Err("Windows batch files (.cmd/.bat) can't run without a shell; call the program's .exe or its interpreter directly.".into());
    }
    Ok(())
}

pub fn which(command: &str) -> Result<Option<String>, String> {
    validate_command(command)?;
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    Ok(executable(command, &cwd, &HashMap::new()).map(|p| p.to_string_lossy().into_owned()))
}

struct Process {
    child: Child,
    group: Option<u32>,
}
impl Process {
    fn kill(&mut self) -> Result<(), String> {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        if let Some(group) = self.group {
            // Check ownership before signalling. WNOWAIT leaves the leader
            // unreaped, so neither its PID nor process-group id can be reused.
            if let Err(error) = observe_leader(group) {
                self.group = None;
                return Err(error.to_string());
            }
            if unsafe { libc::kill(-(group as i32), libc::SIGKILL) } != 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(error.to_string());
                }
            }
            return Ok(());
        }
        self.child.start_kill().map_err(|e| e.to_string())
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        if self.group.is_some() {
            let _ = self.kill();
        }
    }
}
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn observe_leader(pid: u32) -> std::io::Result<bool> {
    loop {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        if unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        } == 0
        {
            #[cfg(target_os = "linux")]
            let exited = unsafe { info.si_pid() };
            #[cfg(target_os = "macos")]
            let exited = info.si_pid;
            return Ok(exited == pid as i32);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}
#[cfg(any(target_os = "linux", target_os = "macos"))]
async fn leader_exited(pid: u32) -> Result<(), String> {
    while !observe_leader(pid).map_err(|e| e.to_string())? {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Ok(())
}
async fn cancelled(rx: &mut watch::Receiver<bool>) {
    if *rx.borrow() {
        return;
    }
    let _ = rx.changed().await;
}

struct Captures {
    out: tokio::task::JoinHandle<()>,
    err: tokio::task::JoinHandle<()>,
}
impl Drop for Captures {
    fn drop(&mut self) {
        self.out.abort();
        self.err.abort();
    }
}

async fn capture<R: AsyncRead + Unpin>(
    mut pipe: R,
    result: Arc<Mutex<Vec<u8>>>,
    remaining: Arc<AtomicUsize>,
    truncated: Arc<AtomicBool>,
) {
    let mut buffer = [0u8; 8192];
    loop {
        let count = match pipe.read(&mut buffer).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => {
                truncated.store(true, Ordering::Release);
                break;
            }
        };
        let before = remaining
            .try_update(Ordering::AcqRel, Ordering::Acquire, |left| {
                Some(left.saturating_sub(count))
            })
            .unwrap();
        let take = count.min(before);
        result.lock().unwrap().extend_from_slice(&buffer[..take]);
        if take < count {
            truncated.store(true, Ordering::Release);
        }
    }
}
fn output(bytes: Vec<u8>, truncated: &AtomicBool) -> String {
    let max = bytes.len();
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if text.len() > max {
        let mut end = max;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        truncated.store(true, Ordering::Release);
    }
    text
}

pub(super) async fn run(input: RunInput, permit: RunPermit) -> Result<RunResult, String> {
    validate_command(&input.command)?;
    if input.args.len() > 256
        || input
            .args
            .iter()
            .any(|s| s.len() > 16384 || s.contains('\0'))
        || input.args.iter().map(String::len).sum::<usize>() > 1024 * 1024
    {
        return Err("Process args exceed limits".into());
    }
    if input.env.len() > 128
        || input
            .env
            .iter()
            .map(|(k, v)| k.len() + v.as_ref().map_or(0, String::len))
            .sum::<usize>()
            > 65536
        || input.env.iter().any(|(k, v)| {
            k.is_empty()
                || k.len() > 256
                || k.contains(['=', '\0'])
                || v.as_ref()
                    .is_some_and(|v| v.len() > 16384 || v.contains('\0'))
        })
    {
        return Err("Invalid process environment or env exceeds limits".into());
    }
    let timeout = input.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
    let max = input.max_output_bytes.unwrap_or(DEFAULT_OUTPUT);
    if timeout == 0 || timeout > MAX_TIMEOUT_MS || max == 0 || max > MAX_OUTPUT {
        return Err("Process timeout/output limit is out of range".into());
    }
    let cwd = match input.cwd {
        Some(path) => super::data::absolute_path(&path)?,
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    let program = executable(&input.command, &cwd, &input.env)
        .ok_or_else(|| format!("Command not found on PATH: {}", input.command))?;
    validate_program(&program, cfg!(windows))?;
    let mut cancel = permit.cancellation();
    if *cancel.borrow() {
        return Ok(RunResult {
            code: None,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
            truncated: false,
            cancelled: true,
        });
    }
    let mut command = Command::new(program);
    command
        .args(input.args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(!cfg!(any(target_os = "linux", target_os = "macos")));
    crate::execution::sanitize_appimage_env_tokio(&mut command);
    for (name, value) in input.env {
        if let Some(value) = value {
            command.env(name, value);
        } else {
            command.env_remove(name);
        }
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    command.process_group(0);
    let child = command.spawn().map_err(|e| e.to_string())?;
    let mut process = Process {
        group: child
            .id()
            .filter(|_| cfg!(any(target_os = "linux", target_os = "macos"))),
        child,
    };
    let remaining = Arc::new(AtomicUsize::new(max));
    let truncated = Arc::new(AtomicBool::new(false));
    let stdout = process.child.stdout.take().ok_or("Missing stdout pipe")?;
    let stderr = process.child.stderr.take().ok_or("Missing stderr pipe")?;
    let stdout_bytes = Arc::new(Mutex::new(Vec::new()));
    let stderr_bytes = Arc::new(Mutex::new(Vec::new()));
    let out = tokio::spawn(capture(
        stdout,
        stdout_bytes.clone(),
        remaining.clone(),
        truncated.clone(),
    ));
    let err = tokio::spawn(capture(
        stderr,
        stderr_bytes.clone(),
        remaining,
        truncated.clone(),
    ));
    let mut captures = Captures { out, err };
    enum End {
        Exited,
        Cancelled,
        TimedOut,
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let ended = tokio::select! { biased; _=cancelled(&mut cancel)=>End::Cancelled,_=tokio::time::sleep(Duration::from_millis(timeout))=>End::TimedOut,result=leader_exited(process.group.unwrap())=>{result?;End::Exited} };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let ended = tokio::select! { biased; _=cancelled(&mut cancel)=>End::Cancelled,_=tokio::time::sleep(Duration::from_millis(timeout))=>End::TimedOut,result=process.child.wait()=>{result.map_err(|e|e.to_string())?;End::Exited} };
    // Reap only after signalling the still-owned Unix group. A setsid child
    // has deliberately left that group and remains independent.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    process.kill()?;
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    if !matches!(ended, End::Exited) {
        process.kill()?;
    }
    let status = process.child.wait().await;
    process.group = None;
    let status = status.map_err(|e| e.to_string())?;
    let collected = tokio::time::timeout(Duration::from_millis(100), async {
        tokio::join!(&mut captures.out, &mut captures.err)
    })
    .await;
    match collected {
        Ok((out, err)) => {
            out.map_err(|e| e.to_string())?;
            err.map_err(|e| e.to_string())?;
        }
        Err(_) => {
            truncated.store(true, Ordering::Release);
        }
    }
    drop(captures);
    let stdout = stdout_bytes.lock().unwrap().clone();
    let stderr = stderr_bytes.lock().unwrap().clone();
    Ok(RunResult {
        code: status.code(),
        stdout: output(stdout, &truncated),
        stderr: output(stderr, &truncated),
        timed_out: matches!(ended, End::TimedOut),
        cancelled: matches!(ended, End::Cancelled),
        truncated: truncated.load(Ordering::Acquire),
    })
}

#[cfg(test)]
mod tests {
    use super::super::PluginHostState;
    use super::*;
    fn input(mode: &str, rest: &[&str]) -> RunInput {
        RunInput {
            command: "python3".into(),
            args: std::iter::once(format!(
                "{}/tests/fixtures/plugin-host-process.py",
                env!("CARGO_MANIFEST_DIR")
            ))
            .chain(std::iter::once(mode.into()))
            .chain(rest.iter().map(|v| v.to_string()))
            .collect(),
            cwd: None,
            env: HashMap::new(),
            timeout_ms: Some(5000),
            max_output_bytes: None,
            workspace_id: None,
        }
    }
    fn permit(host: &PluginHostState) -> RunPermit {
        host.acquire(0, "demo", "test").unwrap();
        host.begin("demo", "test").unwrap()
    }
    #[test]
    fn plugin_host_resolves_path_and_windows_pathext_in_order() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first");
        let second = root.path().join("second");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        for file in [
            first.join("tool.CMD"),
            first.join("tool.EXE"),
            second.join("tool.EXE"),
        ] {
            std::fs::write(file, "fixture").unwrap();
        }
        assert_eq!(
            resolve(
                "tool",
                [first.clone(), second],
                root.path(),
                ".EXE;.CMD",
                true
            ),
            Some(first.join("tool.EXE"))
        );
        assert_eq!(
            resolve("tool", [first.clone()], root.path(), ".CMD;.EXE", true),
            Some(first.join("tool.CMD"))
        );
        assert_eq!(
            resolve("first/tool.CMD", [], root.path(), ".EXE", true),
            Some(first.join("tool.CMD"))
        );
        assert_eq!(
            resolve(
                first.join("tool.EXE").to_str().unwrap(),
                [],
                root.path(),
                ".CMD",
                true
            ),
            Some(first.join("tool.EXE"))
        );
        assert!(resolve("missing", [first], root.path(), ".EXE;.CMD", true).is_none());
    }
    #[test]
    fn plugin_host_windows_execution_alias_is_an_executable_candidate() {
        assert!(windows_candidate(0x400)); // FILE_ATTRIBUTE_REPARSE_POINT
        assert!(windows_candidate(0x80)); // ordinary file, including zero bytes
        assert!(!windows_candidate(0x410)); // directory reparse point
    }
    #[test]
    fn plugin_host_windows_batch_files_never_start_an_implicit_shell() {
        assert!(validate_program(Path::new("tool.CMD"), true).is_err());
        assert!(validate_program(Path::new("tool.bat"), true).is_err());
        assert!(validate_program(Path::new("tool.exe"), true).is_ok());
    }
    #[cfg(unix)]
    #[test]
    fn plugin_host_lookup_uses_only_supplied_path_and_executable_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let local = root.path().join(".local/bin");
        std::fs::create_dir_all(&local).unwrap();
        let tool = local.join("fresh-cli");
        std::fs::write(&tool, "fixture").unwrap();
        assert!(resolve("fresh-cli", [local.clone()], root.path(), "", false).is_none());
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(resolve(
            "fresh-cli",
            [root.path().join("usr/bin")],
            root.path(),
            "",
            false
        )
        .is_none());
        assert_eq!(
            resolve("fresh-cli", [local], root.path(), "", false),
            Some(tool)
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn plugin_host_process_argv_env_cwd_caps_and_timeout() {
        let host = PluginHostState::default();
        let root = tempfile::tempdir().unwrap();
        let mut request = input("echo", &["$(no shell)", "spaces stay together"]);
        request.cwd = Some(root.path().to_str().unwrap().into());
        request
            .env
            .insert("PLUGIN_HOST_OVERRIDE".into(), Some("override".into()));
        request.env.insert("PLUGIN_HOST_REMOVED".into(), None);
        // Remove an actually inherited variable without mutating the test process.
        request.env.insert("HOME".into(), None);
        let result = run(request, permit(&host)).await.unwrap();
        let value: serde_json::Value = serde_json::from_str(result.stdout.trim()).unwrap();
        assert_eq!(
            value["args"],
            serde_json::json!(["$(no shell)", "spaces stay together"])
        );
        assert_eq!(value["cwd"], root.path().to_str().unwrap());
        assert_eq!(value["override"], "override");
        assert!(value["removed"].is_null());
        assert!(value["home"].is_null());
        assert!(value["path"].is_string());
        assert_eq!(result.stderr.trim(), "stderr intact");
        assert_eq!(result.code, Some(0));
        let mut request = input("output", &[]);
        request.max_output_bytes = Some(1024);
        let result = run(request, permit(&host)).await.unwrap();
        assert_eq!(result.stdout.len() + result.stderr.len(), 1024);
        assert!(result.truncated);
        let mut request = input("tree", &[root.path().join("timeout").to_str().unwrap()]);
        request.timeout_ms = Some(150);
        let result = run(request, permit(&host)).await.unwrap();
        assert!(result.timed_out);
        assert!(!result.cancelled);
        let mut request = input("echo", &[]);
        request.cwd = Some("relative".into());
        assert!(run(request, permit(&host)).await.is_err());
    }
    #[cfg(target_os = "linux")]
    async fn wait_for(path: &Path) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !path.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn plugin_host_dispose_kills_grandchild_but_detached_child_survives_without_holding_pipes(
    ) {
        let root = tempfile::tempdir().unwrap();
        let heartbeat = root.path().join("tree");
        let host = Arc::new(PluginHostState::default());
        let request = input("tree", &[heartbeat.to_str().unwrap()]);
        let task = tokio::spawn(run(request, permit(&host)));
        wait_for(&heartbeat).await;
        host.release("demo", "test").await;
        assert!(task.await.unwrap().unwrap().cancelled);
        let stopped = std::fs::metadata(&heartbeat).unwrap().len();
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(std::fs::metadata(&heartbeat).unwrap().len(), stopped);
        let timeout_heartbeat = root.path().join("timeout-tree");
        let mut request = input("tree", &[timeout_heartbeat.to_str().unwrap()]);
        request.timeout_ms = Some(2000);
        let task = tokio::spawn(run(request, permit(&host)));
        wait_for(&timeout_heartbeat).await;
        assert!(task.await.unwrap().unwrap().timed_out);
        let stopped = std::fs::metadata(&timeout_heartbeat).unwrap().len();
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(
            std::fs::metadata(&timeout_heartbeat).unwrap().len(),
            stopped
        );
        let heartbeat = root.path().join("detached");
        let start = std::time::Instant::now();
        let result = run(
            input("detached", &[heartbeat.to_str().unwrap()]),
            permit(&host),
        )
        .await
        .unwrap();
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(result.code, Some(0));
        assert!(result.stdout.contains("launched detached child"));
        assert!(result.truncated);
        wait_for(&heartbeat).await;
        let before = std::fs::metadata(&heartbeat).unwrap().len();
        host.release("demo", "test").await;
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(std::fs::metadata(&heartbeat).unwrap().len() > before);
        // Only this fixture's deliberately detached process is ours to stop.
        let pid: u32 = std::fs::read_to_string(heartbeat.with_extension("pid"))
            .unwrap()
            .parse()
            .unwrap();
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }
    }
}
