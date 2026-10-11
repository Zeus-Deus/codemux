//! Durable, one-shot native agent tasks. SSH authenticates the remote OS user;
//! provider permission modes are not a Codemux-created security sandbox.

pub mod cli;
mod checkout;
mod store;
use checkout::CheckoutIdentity;
mod worker;

use crate::agent_provider::{ProviderChatCapabilities, ProviderKind};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub use store::TaskStore;

mod types;
pub use types::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderAvailability {
    pub provider: ProviderKind,
    pub capabilities: Option<ProviderChatCapabilities>,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskCapabilities {
    pub protocol_version: u32,
    pub providers: Vec<ProviderAvailability>,
    pub workspaces: Vec<super::workspace::Workspace>,
}

/// Called by the single-threaded CLI before constructing Tokio. This pins a
/// server-distributed Claude sidecar for the existing live capability cache.
pub fn initialize_runtime() {
    if std::env::var_os(crate::agent_provider::claude::sidecar_path::SIDECAR_PATH_ENV).is_none() {
        if let Ok(path) = resolve_sidecar() {
            std::env::set_var(
                crate::agent_provider::claude::sidecar_path::SIDECAR_PATH_ENV,
                path,
            );
        }
    }
}
fn sibling_sidecar(exe: &Path) -> Option<PathBuf> {
    let parent = exe.parent()?;
    let qualified = format!(
        "codemux-claude-sidecar-{}",
        crate::agent_provider::claude::sidecar_path::target_triple()
    );
    for folder in [parent.to_path_buf(), parent.join("binaries")] {
        for name in [&qualified, "codemux-claude-sidecar"] {
            let path = folder.join(name);
            if crate::agent_provider::claude::sidecar_path::is_executable(&path) {
                return Some(path);
            }
        }
    }
    None
}
fn resolve_sidecar() -> Result<PathBuf, String> {
    use crate::agent_provider::claude::sidecar_path as sidecar;
    if let Some(explicit) = std::env::var_os(sidecar::SIDECAR_PATH_ENV).filter(|s| !s.is_empty()) {
        let path = PathBuf::from(explicit);
        return if sidecar::is_executable(&path) {
            Ok(path)
        } else {
            Err("Claude sidecar override is missing or not executable; repair CODEMUX_CLAUDE_SIDECAR_PATH on this host".into())
        };
    }
    if let Some(path) = std::env::current_exe()
        .ok()
        .and_then(|p| sibling_sidecar(&p))
    {
        return Ok(path);
    }
    let path=sidecar::resolve_sidecar_path().map_err(|e|format!("Claude runtime unavailable: {e}. Install the matching executable codemux-claude-sidecar beside codemux-remote."))?;
    if !sidecar::is_executable(&path) {
        return Err("Claude sidecar is not executable; repair its host installation".into());
    }
    Ok(path)
}
async fn provider_capabilities(provider: ProviderKind) -> Result<ProviderChatCapabilities, String> {
    use crate::agent_provider::{claude, codex};
    let caps = match provider {
        ProviderKind::Codex => {
            let binary = which::which("codex").map_err(|_| {
                "Codex runtime unavailable: install codex on this host and run codex login"
                    .to_string()
            })?;
            let home = std::env::var_os("CODEX_HOME").map(PathBuf::from);
            codex::capabilities::harvest_codex_capabilities(&binary, home.as_deref())
                .await
                .map_err(|e| e.to_command_string())?
        }
        ProviderKind::Claude => {
            let sidecar = resolve_sidecar()?;
            let binary = which::which("claude").map_err(|_| {
                "Claude CLI is not installed on this host; install and authenticate Claude Code"
                    .to_string()
            })?;
            let installed = claude::auth::probe_installed(&sidecar, Some(&binary))
                .await
                .map_err(|e| e.to_string())?;
            if !installed.installed {
                return Err("Claude CLI is unavailable; repair the host installation".into());
            }
            match claude::auth::probe_authenticated(&sidecar,Some(&binary)).await.map_err(|e|e.to_string())? {
                claude::auth::AuthStatus::Authenticated => {},
                claude::auth::AuthStatus::Unauthenticated {message} => return Err(message),
                claude::auth::AuthStatus::Unknown {..} => return Err("Claude authentication could not be verified on this host; authenticate Claude Code and retry".into()),
            }
            // This cache returns an error when both live paths fail. Never use
            // claude_fallback_capabilities or any maintained model catalogue.
            claude::capabilities::ClaudeCapabilityCache::new()
                .get_or_harvest()
                .await
                .map_err(|e| e.to_command_string())?
        }
        _ => {
            return Err("remote delegation currently supports native Codex and Claude only".into())
        }
    };
    if caps.models.is_empty() {
        return Err("host runtime returned no available models".into());
    }
    Ok(caps)
}
fn workspaces(root: &Path) -> Result<Vec<super::workspace::Workspace>, String> {
    super::workspace::WorkspaceStore::open(
        &super::config::database_path(root),
        super::manifest::current_host_id(),
        super::config::workspaces_root(root),
    )
    .map_err(|e| e.to_string())?
    .list()
    .map_err(|e| e.to_string())
}
pub async fn capabilities(root: &Path) -> Result<TaskCapabilities, String> {
    let (codex, claude) = tokio::join!(
        availability(ProviderKind::Codex),
        availability(ProviderKind::Claude)
    );
    Ok(TaskCapabilities {
        protocol_version: 1,
        providers: vec![codex, claude],
        workspaces: workspaces(root)?,
    })
}
async fn availability(provider: ProviderKind) -> ProviderAvailability {
    match tokio::time::timeout(Duration::from_secs(35), provider_capabilities(provider)).await {
        Ok(Ok(capabilities)) => ProviderAvailability {
            provider,
            capabilities: Some(capabilities),
            error: None,
        },
        Ok(Err(error)) => ProviderAvailability {
            provider,
            capabilities: None,
            error: Some(error),
        },
        Err(_) => ProviderAvailability {
            provider,
            capabilities: None,
            error: Some(
                "Host provider capability probe timed out; no fallback models are advertised"
                    .into(),
            ),
        },
    }
}
async fn prepare(root: &Path, request: &LaunchRequest) -> Result<CheckoutIdentity, String> {
    validate_shape(request)?;
    let bundle = TaskCapabilities {
        protocol_version: 1,
        providers: vec![availability(request.provider).await],
        workspaces: workspaces(root)?,
    };
    validate_launch(request, &bundle)
}
fn validate_shape(request: &LaunchRequest) -> Result<(), String> {
    store::validate_id(&request.id)?;
    if !matches!(request.provider, ProviderKind::Codex | ProviderKind::Claude) {
        return Err(
            "unsupported remote provider; only native Codex and Claude are qualified".into(),
        );
    }
    if request.prompt.trim().is_empty() || request.prompt.len() > 32_000 {
        return Err("prompt must be nonempty and at most 32000 bytes".into());
    }
    if request.parent_thread_id.trim().is_empty()
        || request.parent_thread_id.len() > 200
        || request.parent_label.len() > 512
    {
        return Err("parent thread identity or label is missing or too long".into());
    }
    if !Path::new(&request.workspace_path).is_absolute() {
        return Err("workspace_path must be an absolute registered checkout".into());
    }
    Ok(())
}
fn validate_launch(
    request: &LaunchRequest,
    capabilities: &TaskCapabilities,
) -> Result<CheckoutIdentity, String> {
    validate_shape(request)?;
    let path = Path::new(&request.workspace_path);
    if !path.is_dir() {
        return Err("registered checkout is missing or is not a directory".into());
    }
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("checkout path: {e}"))?;
    let workspace = capabilities
        .workspaces
        .iter()
        .find(|w| {
            w.path == request.workspace_path
                && Path::new(&w.path).canonicalize().ok().as_ref() == Some(&canonical)
        })
        .ok_or("checkout is not registered in this remote host's workspace registry")?;
    let git = std::process::Command::new("git")
        .args([
            "-C",
            &request.workspace_path,
            "rev-parse",
            "--show-toplevel",
        ])
        .output()
        .map_err(|e| format!("validate checkout: {e}"))?;
    if !git.status.success()
        || Path::new(String::from_utf8_lossy(&git.stdout).trim())
            .canonicalize()
            .ok()
            .as_ref()
            != Some(&canonical)
    {
        return Err(
            "workspace_path must be the root of an existing Git checkout, not a subdirectory"
                .into(),
        );
    }
    let entry = capabilities
        .providers
        .iter()
        .find(|p| p.provider == request.provider)
        .ok_or("provider was not probed on the receiver host")?;
    if let Some(error) = &entry.error {
        return Err(error.clone());
    }
    let caps = entry
        .capabilities
        .as_ref()
        .ok_or("provider live capabilities are unavailable")?;
    if !caps
        .permission_modes
        .iter()
        .any(|mode| mode.value == request.permission_mode)
    {
        return Err("permission_mode is not supported by the receiver's native provider".into());
    }
    let model = match request.model.as_deref() {
        Some(id) => Some(
            caps.models
                .iter()
                .find(|model| model.id == id)
                .ok_or("model is not in the receiver's live capability catalogue")?,
        ),
        None => None,
    };
    if let Some(effort) = request.effort.as_deref() {
        let model = model.ok_or("an explicit live model is required when selecting effort")?;
        if !model.effort_levels.iter().any(|level| level == effort)
            || model
                .prompt_injected_effort_levels
                .iter()
                .any(|level| level == effort)
        {
            return Err("effort is not natively supported by the receiver model".into());
        }
    }
    let identity=CheckoutIdentity::capture(workspace.id.clone(),path)?;
    if identity.canonical_path!=canonical {return Err("Checkout changed during validation".into());}
    Ok(identity)
}

pub async fn launch(root: &Path, request: LaunchRequest) -> Result<TaskRead, String> {
    let store = TaskStore::open(root)?;
    if store.existing(&request)?.is_some() {
        store.reconcile(&request.id)?;
        return store.read(&request.id, 0);
    }
    let checkout=prepare(root, &request).await?;
    launch_admitted(root, &store, &request, Some(&checkout), || spawn_worker(root, &request.id))?;
    store.read(&request.id, 0)
}
fn launch_admitted(
    root: &Path,
    store: &TaskStore,
    request: &LaunchRequest,
    identity: Option<&CheckoutIdentity>,
    spawn: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let _ = root;
    if store.admit_bound(request, identity)? {
        if let Err(error) = spawn() {
            store.update(&request.id, |task| {
                task.status = TaskStatus::Failed;
                task.error = Some(format!(
                    "Detached worker launch failed before execution: {error}"
                ));
                task.activity = None;
            })?;
        }
    }
    Ok(())
}
fn spawn_worker(root: &Path, id: &str) -> Result<(), String> {
    use std::os::unix::{fs::OpenOptionsExt, process::CommandExt};
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(root.join("tasks").join(format!("{id}.log")))
        .map_err(|e| e.to_string())?;
    let mut command =
        std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command
        .args(["task", "run", "--id", id, "--state-dir"])
        .arg(root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(
            log.try_clone().map_err(|e| e.to_string())?,
        ))
        .stderr(std::process::Stdio::from(log));
    // SAFETY: setsid is async-signal-safe. No allocation or locks in pre_exec.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
pub async fn read(root: &Path, id: &str, after: i64, wait_ms: u64) -> Result<TaskRead, String> {
    let store = TaskStore::open(root)?;
    store.reconcile(id)?;
    let initial = store.read(id, after)?;
    if !initial.events.is_empty() || initial.task.status.is_terminal() || wait_ms == 0 {
        return Ok(initial);
    }
    let deadline = tokio::time::Instant::now() + Duration::from_millis(wait_ms.min(15_000));
    loop {
        tokio::time::sleep(
            Duration::from_millis(100)
                .min(deadline.saturating_duration_since(tokio::time::Instant::now())),
        )
        .await;
        store.reconcile(id)?;
        let current = store.read(id, after)?;
        if !current.events.is_empty()
            || current.task.updated_at != initial.task.updated_at
            || tokio::time::Instant::now() >= deadline
        {
            return Ok(current);
        }
    }
}
pub fn cancel(root: &Path, id: &str) -> Result<CancelReceipt, String> {
    let store = TaskStore::open(root)?;
    if !store.cancel_or_fence(id)? {
        return Ok(CancelReceipt::Absent(CancelFence{id:id.into(),absent_and_fenced:true}));
    }
    store.reconcile(id)?;
    Ok(CancelReceipt::Task(store.read(id, 0)?))
}
pub fn respond(root: &Path, id: &str, response: RespondRequest) -> Result<TaskRead, String> {
    let store = TaskStore::open(root)?;
    store.reconcile(id)?;
    store.respond(id, &response)?;
    store.read(id, 0)
}
pub async fn run(root: &Path, id: &str) -> Result<(), String> {
    use crate::agent_provider::{
        claude::{ClaudeAgentProvider, ClaudeProviderConfig},
        codex::{CodexAgentProvider, CodexProviderConfig},
        AgentProvider,
    };
    let store = Arc::new(TaskStore::open(root)?);
    let _lease = store.claim(id)?;
    let request = store.snapshot(id)?.request;
    let setup = async {
        if store.snapshot(id)?.cancel_requested {
            // No native session has been created by this one-shot claim.
            store.update(id, |t| {
                t.status = TaskStatus::Cancelled;
                t.activity = None;
            })?;
            return Ok(None);
        }
        let checkout = prepare(root, &request).await?;
        if store.checkout(id)?.as_ref()!=Some(&checkout) {return Err("Checkout no longer matches its durable admission identity".into());}
        let workspace_id=checkout.workspace_id;
        let provider: Arc<dyn AgentProvider> = match request.provider {
            ProviderKind::Codex => Arc::new(CodexAgentProvider::new(CodexProviderConfig {
                codex_binary: which::which("codex").map_err(|e| e.to_string())?,
                codex_home: std::env::var_os("CODEX_HOME").map(PathBuf::from),
                event_channel_capacity: 65_536,
                ..Default::default()
            })),
            ProviderKind::Claude => Arc::new(
                ClaudeAgentProvider::new(ClaudeProviderConfig {
                    sidecar_binary: Some(resolve_sidecar()?),
                    claude_binary: Some(which::which("claude").map_err(|e| e.to_string())?),
                    event_channel_capacity: 65_536,
                    mcp_registry: None,
                })
                .await
                .map_err(|e| e.to_string())?,
            ),
            _ => return Err("unsupported remote native provider".to_string()),
        };
        Ok(Some((provider, workspace_id)))
    }
    .await;
    match setup {
        Ok(Some((provider, workspace_id))) => {
            worker::execute_claimed(store, id, provider, workspace_id).await
        }
        Ok(None) => Ok(()),
        Err(error) => {
            store.update(id, |task| {
                task.status = TaskStatus::Failed;
                task.error = Some(error.clone());
                task.activity = None;
            })?;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detached_spawn_observes_durable_intent_and_retry_or_failure_never_respawns() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = TaskStore::open(dir.path()).unwrap();
        let request = request();
        let count = std::cell::Cell::new(0);
        launch_admitted(dir.path(), &store, &request, None, || {
            assert_eq!(
                TaskStore::open(dir.path())
                    .unwrap()
                    .snapshot(&request.id)
                    .unwrap()
                    .request,
                request
            );
            count.set(count.get() + 1);
            Err("fixture executable missing".into())
        })
        .unwrap();
        assert_eq!(
            store.snapshot(&request.id).unwrap().status,
            TaskStatus::Failed
        );
        launch_admitted(dir.path(), &store, &request, None, || {
            count.set(count.get() + 1);
            Ok(())
        })
        .unwrap();
        assert_eq!(count.get(), 1);
        let mut conflict = request.clone();
        conflict.permission_mode = "danger-full-access".into();
        assert!(launch_admitted(dir.path(), &store, &conflict, None, || {
            panic!("conflict cannot spawn")
        })
        .is_err());
    }

    #[tokio::test]
    async fn r2_validated_checkout_replacement_cannot_start_native_elsewhere() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::TempDir::new().unwrap();
        let a=dir.path().join("A"); let b=dir.path().join("B"); let current=dir.path().join("current");
        for path in [&a,&b] {std::fs::create_dir(path).unwrap();assert!(std::process::Command::new("git").args(["init","-q"]).arg(path).status().unwrap().success());}
        symlink(&a,&current).unwrap();
        let workspace=super::super::workspace::WorkspaceStore::open(&super::super::config::database_path(dir.path()),"fixture".into(),dir.path().join("workspaces")).unwrap().create(None,current.to_string_lossy().into(),None,None).unwrap();
        let caps=crate::agent_provider::codex::capabilities::build_capabilities(vec![serde_json::from_value(serde_json::json!({"id":"fixture","model":"fixture","displayName":"Fixture","hidden":false,"isDefault":true,"defaultReasoningEffort":"medium","supportedReasoningEfforts":[]})).unwrap()]);
        let bundle=TaskCapabilities{protocol_version:1,providers:vec![ProviderAvailability{provider:ProviderKind::Codex,capabilities:Some(caps),error:None}],workspaces:vec![workspace]};
        let mut request=request(); request.workspace_path=current.to_string_lossy().into();
        let validated=validate_launch(&request,&bundle).unwrap();
        let store=Arc::new(TaskStore::open(dir.path()).unwrap());
        launch_admitted(dir.path(),&store,&request,Some(&validated),||Ok(())).unwrap();
        std::fs::remove_file(&current).unwrap(); symlink(&b,&current).unwrap();
        let provider=worker::tests::fixture(&dir,serde_json::json!([]));
        // Exercise the actual startup path with a persisted validation epoch.
        let worker=tokio::spawn({let store=store.clone();let id=request.id.clone();async move {worker::execute(store,&id,provider,validated.workspace_id).await}});
        let trace=dir.path().join("trace.jsonl");
        let deadline=tokio::time::Instant::now()+Duration::from_secs(2);
        while !worker.is_finished() && !trace.exists() && tokio::time::Instant::now()<deadline {tokio::time::sleep(Duration::from_millis(10)).await;}
        store.cancel(&request.id).unwrap();
        let outcome=tokio::time::timeout(Duration::from_secs(10),worker).await.unwrap().unwrap();
        assert!(!trace.exists(),"checkout replacement reached native startup in another checkout: {outcome:?}");
        assert!(outcome.is_err());
    }


    #[tokio::test]
    async fn r2_native_startup_directory_handle_survives_name_replacement() {
        let dir=tempfile::TempDir::new().unwrap();let a=dir.path().join("A");let moved=dir.path().join("moved-A");
        std::fs::create_dir(&a).unwrap(); assert!(std::process::Command::new("git").args(["init","-q"]).arg(&a).status().unwrap().success());
        let registry=super::super::workspace::WorkspaceStore::open(&super::super::config::database_path(dir.path()),"fixture".into(),dir.path().join("workspaces")).unwrap();
        let registered=registry.create(None,a.to_string_lossy().into(),None,None).unwrap();
        let identity=CheckoutIdentity::capture(registered.id.clone(),&a).unwrap();
        let store=Arc::new(TaskStore::open(dir.path()).unwrap());let mut request=request();request.workspace_path=a.to_string_lossy().into();
        store.admit_bound(&request,Some(&identity)).unwrap();
        assert_eq!(TaskStore::open(dir.path()).unwrap().checkout(&request.id).unwrap(),Some(identity));
        let provider=worker::tests::fixture(&dir,serde_json::json!([
            {"after":"turn/start","delay_ms":20,"emit":"notification","method":"turn/completed","params":{"threadId":"c-1","turnId":"t-1","status":"succeeded"}}
        ]));
        let wrapper=dir.path().join("codex-fixture");let original=std::fs::read_to_string(&wrapper).unwrap();
        // This executes inside the real spawned native CLI, AFTER chdir of
        // the provider process but BEFORE native thread/start receives cwd.
        let replacement=format!("mv '{}' '{}'\nmkdir '{}'\nprintf 'native in validated inode' > native-started\nexec '",a.display(),moved.display(),a.display());
        std::fs::write(&wrapper,original.replace("exec '",&replacement)).unwrap();
        let outcome=worker::execute(store.clone(),&request.id,provider,registered.id).await;
        assert!(outcome.is_ok(),"{outcome:?}");
        assert!(moved.join("native-started").exists());
        assert!(!a.join("native-started").exists(),"replacement checkout received native startup side effect");
        let calls=std::fs::read_to_string(dir.path().join("trace.jsonl")).unwrap();
        let start:serde_json::Value=calls.lines().map(|l|serde_json::from_str::<serde_json::Value>(l).unwrap()).find(|v|v["method"]=="thread/start").unwrap();
        assert!(start["params"]["cwd"].as_str().unwrap().starts_with("/proc/"));
        assert_eq!(store.snapshot(&request.id).unwrap().status,TaskStatus::Completed);
    }

    #[test]
    fn server_distribution_discovers_executable_sibling_sidecars() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let executable = dir.path().join("codemux-remote");
        assert!(sibling_sidecar(&executable).is_none());
        let sidecar = dir.path().join("codemux-claude-sidecar");
        std::fs::write(&sidecar, "fixture").unwrap();
        std::fs::set_permissions(&sidecar, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(sibling_sidecar(&executable).is_none());
        std::fs::set_permissions(&sidecar, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(sibling_sidecar(&executable), Some(sidecar));
    }
    pub(super) fn request() -> LaunchRequest {
        LaunchRequest {
            id: uuid::Uuid::new_v4().to_string(),
            parent_thread_id: "parent".into(),
            parent_label: "parent label".into(),
            workspace_path: "/fixture/checkout".into(),
            prompt: "fixture task".into(),
            provider: ProviderKind::Codex,
            model: None,
            permission_mode: "read-only".into(),
            effort: None,
        }
    }
    #[test]
    fn launch_requires_registered_existing_checkout_and_live_model_mode_effort() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(dir.path())
            .status()
            .unwrap()
            .success());
        let workspace = super::super::workspace::WorkspaceStore::open(
            &dir.path().join("registry.db"),
            "fixture".into(),
            dir.path().join("workspaces"),
        )
        .unwrap()
        .create(None, dir.path().to_string_lossy().into(), None, None)
        .unwrap();
        let caps=crate::agent_provider::codex::capabilities::build_capabilities(vec![serde_json::from_value(serde_json::json!({
            "id":"fixture-model","model":"fixture-model","displayName":"Fixture","hidden":false,"isDefault":true,
            "defaultReasoningEffort":"medium","supportedReasoningEfforts":[{"reasoningEffort":"medium","description":"Fixture"}]
        })).unwrap()]);
        let availability = TaskCapabilities {
            protocol_version: 1,
            providers: vec![ProviderAvailability {
                provider: ProviderKind::Codex,
                capabilities: Some(caps),
                error: None,
            }],
            workspaces: vec![workspace.clone()],
        };
        let mut launch = request();
        launch.workspace_path = workspace.path.clone();
        launch.model = Some("fixture-model".into());
        launch.effort = Some("medium".into());
        let valid = validate_launch(&launch, &availability);
        assert!(
            valid.is_ok(),
            "a registered checkout and live native options must validate: {valid:?}"
        );
        assert_eq!(valid.unwrap().workspace_id, workspace.id);
        launch.permission_mode = "invented mode".into();
        assert!(validate_launch(&launch, &availability).is_err());
        launch.permission_mode = "read-only".into();
        launch.model = Some("invented model".into());
        assert!(validate_launch(&launch, &availability).is_err());
        launch.model = Some("fixture-model".into());
        launch.effort = Some("invented effort".into());
        assert!(validate_launch(&launch, &availability).is_err());
        launch.effort = None;
        launch.provider = ProviderKind::Hermes;
        assert!(validate_launch(&launch, &availability).is_err());
        launch.provider = ProviderKind::Codex;
        launch.workspace_path = dir.path().join("workspaces").to_string_lossy().into();
        assert!(validate_launch(&launch, &availability).is_err());
        launch.workspace_path = workspace.path;
        launch.prompt = "x".repeat(32_001);
        assert!(validate_launch(&launch, &availability).is_err());
        let mut json = serde_json::to_value(request()).unwrap();
        json["env"] = serde_json::json!({"FAKE":"override"});
        assert!(
            serde_json::from_value::<LaunchRequest>(json).is_err(),
            "wire never accepts environment or arbitrary provider config"
        );
    }
}
