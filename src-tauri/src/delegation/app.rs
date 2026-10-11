use super::{
    journal::Journal,
    permissions,
    transport::{Operation, SshTransport, TaskTransport},
    types::*,
};
use crate::{
    agent_provider::{ProviderKind, SessionStatus},
    database::DatabaseStore,
    state::{find_agent_chat_pane_id, AppStateStore},
};
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, Runtime};

pub fn admit<R: Runtime>(app: &AppHandle<R>, parent: &Parent, input: &DelegateTaskInput) -> Result<LocalTask, String> {
    match app.try_state::<super::coordinator::Coordinator>() {
        Some(coordinator) => coordinator.with_admission(|| admit_open(app, parent, input)),
        None => admit_open(app, parent, input),
    }
}
fn admit_open<R: Runtime>(
    app: &AppHandle<R>,
    parent: &Parent,
    input: &DelegateTaskInput,
) -> Result<LocalTask, String> {
    if input.prompt.trim().is_empty()
        || input.prompt.len() > 32000
        || input.title.as_ref().is_some_and(|t| t.len() > 512)
    {
        return Err(
            "A self-contained task requires a nonempty prompt <=32000 bytes and title <=512 bytes"
                .into(),
        );
    }
    let store = journal(app)?;
    let grant = store
        .grants(&parent.workspace_id)?
        .into_iter()
        .find(|g| g.id == input.target_id)
        .ok_or("Target is not authorized for this parent workspace")?;
    validate_grant(app, parent, &grant)?;
    let info = store.host_info(grant.host_id)?;
    let workspace = info
        .workspaces
        .iter()
        .find(|w| w.path == grant.workspace_path)
        .ok_or("Authorized existing checkout is absent from the receiver registry")?;
    let caps = info
        .providers
        .iter()
        .find(|p| p.provider == grant.provider && p.error.is_none())
        .and_then(|p| p.capabilities.as_ref())
        .ok_or("Receiver capabilities unavailable; probe Run on host again")?;
    if !caps
        .permission_modes
        .iter()
        .any(|m| m.value == grant.permission_mode)
    {
        return Err("Authorized receiver mode is unavailable".into());
    }
    let model = input
        .model
        .as_ref()
        .map(|id| {
            caps.models
                .iter()
                .find(|m| &m.id == id)
                .ok_or("Model is not in the receiving runtime's capability catalogue")
        })
        .transpose()?;
    if let Some(effort) = &input.effort {
        if !model.is_some_and(|m| {
            m.effort_levels.contains(effort) && !m.prompt_injected_effort_levels.contains(effort)
        }) {
            return Err("Effort is not natively available on the selected receiver model".into());
        }
    }
    store.admit(parent, &grant, &workspace.id, input)
}
pub fn start<R: Runtime>(app: &AppHandle<R>, id: &str) {
    if let Some(coordinator) = app.try_state::<super::coordinator::Coordinator>() {
        coordinator.start(app.clone(), id.into());
    }
}
// Only adapters construct this in process. Never deserialized from MCP params.
#[derive(Debug, Clone)]
pub(crate) struct NativeActor {
    pub thread_id: String,
    pub provider: ProviderKind,
    pub workspace_id: Option<String>,
    pub session_id: String,
    pub permission_mode: Option<String>,
    pub permit: super::authority::NativePermit,
}
pub(crate) async fn native_call<R: Runtime>(
    app: AppHandle<R>,
    actor: NativeActor,
    name: String,
    args: Value,
) -> Result<Value, String> {
    if !matches!(actor.provider, ProviderKind::Codex | ProviderKind::Claude) {
        return Err("Native delegation requires a qualified Claude/Codex adapter".into());
    }
    let parent = parent_for_thread(&app, &actor.thread_id)?;
    if parent.provider != actor.provider
        || actor.workspace_id.as_deref() != Some(&parent.workspace_id)
    {
        return Err("Native caller ownership does not match the canonical parent".into());
    }
    let registry = app.state::<crate::commands::agent_chat::ProviderRegistry>();
    let provider = crate::commands::agent_chat::lookup_provider(&registry, actor.provider).await?;
    let live = provider
        .list_sessions()
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .any(|s| {
            s.thread_id.0 == actor.thread_id
                && s.session_id.0 == actor.session_id
                && matches!(
                    s.status,
                    SessionStatus::Ready
                        | SessionStatus::Running { .. }
                        | SessionStatus::WaitingApproval { .. }
                )
        });
    if !live {
        return Err("Native caller is stale, ended, or no longer owned by this adapter".into());
    }
    // Revocation and durable admission use the same synchronous fence.
    // Re-read canonical ownership AFTER provider/registry awaits.
    let _admission = actor.permit.admit()?;
    let mut parent = parent_for_thread(&app, &actor.thread_id)?;
    if parent.provider != actor.provider
        || actor.workspace_id.as_deref() != Some(&parent.workspace_id)
    {
        return Err("Native caller ownership changed before admission".into());
    }
    let native_mode = actor
        .permission_mode
        .ok_or("Native caller's effective permission ceiling is unknown")?;
    let store = journal(&app)?;
    let compatible = |g: &Grant| {
        permissions::permits(
            parent.provider,
            &native_mode,
            g.provider,
            &g.permission_mode,
        ) && permissions::permits(
            parent.provider,
            &parent.permission_mode,
            g.provider,
            &g.permission_mode,
        )
    };
    let _notice = if matches!(name.as_str(),"delegate_task"|"task_cancel") {Some(MutationNotice::new(&app,&parent)?)} else {None};
    match name.as_str() {
        "delegation_targets" => value(
            store
                .grants(&parent.workspace_id)?
                .into_iter()
                .filter(|g| compatible(g) && validate_grant(&app, &parent, g).is_ok())
                .collect::<Vec<_>>(),
        ),
        "delegate_task" => {
            let input: DelegateTaskInput =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            let grant = store
                .grants(&parent.workspace_id)?
                .into_iter()
                .find(|g| g.id == input.target_id)
                .ok_or("Target is not authorized")?;
            if !compatible(&grant) {
                return Err("Target exceeds the actual adapter permission ceiling".into());
            }
            parent.permission_mode = native_mode;
            let task = admit(&app, &parent, &input)?;
            start(&app, &task.id);
            value(project_task(&app,&store,&parent,task)?)
        }
        "task_status" | "task_cancel" => {
            if args
                .as_object()
                .is_none_or(|o| o.keys().any(|k| k != "task_id" && k != "cursor"))
            {
                return Err("Unexpected native task tool fields".into());
            }
            let id = string(&args, "task_id")?;
            owned(&store, &parent, &id)?;
            if name == "task_cancel" {
                let task = store.cancel(&id)?;
                start(&app, &id);
                value(project_task(&app,&store,&parent,task)?)
            } else {
                let mut read=store.read(&id,args.get("cursor").and_then(Value::as_i64).unwrap_or(0))?;
                read.task=project_task(&app,&store,&parent,read.task)?;
                value(read)
            }
        }
        _ => Err("Unknown native delegation tool".into()),
    }
}
pub fn user_activity<R: Runtime>(
    app: &AppHandle<R>,
    thread: &str,
    stop: bool,
) -> Result<(), String> {
    super::authority::NativeAuthority::invalidate_thread(thread);
    if let Some(store) = app.try_state::<Arc<Journal>>() {
        let _notice = store.tasks()?.iter().find(|task|task.parent_thread_id==thread)
            .map(|task|MutationNotice::for_task(app,task)).transpose()?;
        store.suppress_parent(thread, stop)?;
        for task in store
            .tasks()?
            .into_iter()
            .filter(|t| t.parent_thread_id == thread && t.cancel_requested)
        {
            start(app, &task.id);
        }

    }
    Ok(())
}
pub fn guard_wake<R: Runtime>(
    app: &AppHandle<R>,
    thread: &str,
    nonce: Option<&str>,
) -> Result<(), String> {
    let id = nonce
        .and_then(|s| s.strip_prefix("delegation:"))
        .ok_or("Missing stable delegation delivery correlation")?;
    let store = journal(app)?;
    let task = store.task(id)?;
    if task.parent_thread_id != thread
        || task.cancel_requested
        || task.wake_state != WakeState::Delivering
    {
        return Err("Delegation delivery was stopped or superseded".into());
    }
    let parent = parent_for_thread(app, thread)?;
    owned(&store, &parent, id)?;
    validate_grant(app, &parent, &store.grant_for_task(id)?)
}

struct WakeDispatch<R: Runtime> {
    app: AppHandle<R>,
    store: Arc<Journal>,
    id: String,
    attempt: String,
    permit: super::authority::DispatchPermit,
}
impl<R: Runtime> std::fmt::Debug for WakeDispatch<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WakeDispatch")
            .field("task", &self.id)
            .finish()
    }
}
impl<R: Runtime> crate::json_rpc_child::dispatch::DispatchGuard for WakeDispatch<R> {
    fn register(&self, waker: &std::task::Waker) {
        self.permit.waker.register(waker);
    }
    fn poll_first(
        &self,
        write: &mut dyn FnMut() -> crate::json_rpc_child::dispatch::WritePoll,
    ) -> Result<crate::json_rpc_child::dispatch::WritePoll, String> {
        let _activity = self.permit.admit()?;
        with_parent_admission(&self.app, &self.store.task(&self.id)?.parent_thread_id, |parent,host| {
            self.store.poll_wake(&self.id,&self.attempt, |task,grant| {
                if parent.workspace_id!=task.parent_workspace_id || parent.provider!=task.parent_provider { return Err("Canonical parent ownership changed at native dispatch".into()); }
                validate_grant_at(parent,grant,host)
            },write)
        })
    }
    fn accepted(&self) -> Result<(), String> {
        self.store.accept_delivery(&self.id, &self.attempt)
    }
}
pub(crate) fn dispatch_guard<R: Runtime>(
    app: &AppHandle<R>,
    thread: &str,
    nonce: Option<&str>,
) -> Result<Arc<dyn crate::json_rpc_child::dispatch::DispatchGuard>, String> {
    guard_wake(app, thread, nonce)?;
    let id = nonce
        .and_then(|n| n.strip_prefix("delegation:"))
        .unwrap()
        .to_owned();
    let store = journal(app)?;
    let attempt = store.delivery_attempt(&id)?;
    let permit = super::authority::DispatchPermit::capture(thread)?;
    store.register_dispatch(&permit.waker);
    Ok(Arc::new(WakeDispatch {
        app: app.clone(),
        store,
        id,
        attempt,
        permit,
    }))
}

pub fn journal<R: Runtime>(app: &AppHandle<R>) -> Result<Arc<Journal>, String> {
    app.try_state::<Arc<Journal>>()
        .map(|s| s.inner().clone())
        .ok_or("Delegation journal is unavailable".into())
}
pub fn parent_for_pane<R: Runtime>(app: &AppHandle<R>, pane: &str) -> Result<Parent, String> {
    let state = app.state::<AppStateStore>();
    let (provider, thread) = state
        .agent_chat_pane_thread(pane)
        .ok_or("The pane has no live parent thread")?;
    let parent = parent_for_thread(app, &thread)?;
    if parent.provider != provider
        || state.workspace_id_for_pane(pane).as_deref() != Some(&parent.workspace_id)
        || state
            .find_workspace(&parent.workspace_id)
            .and_then(|w| {
                w.surfaces
                    .iter()
                    .find_map(|s| find_agent_chat_pane_id(&s.root, &thread))
            })
            .map(|p| p.0)
            .as_deref()
            != Some(pane)
    {
        return Err("Parent pane/provider ownership changed".into());
    }
    Ok(parent)
}
pub fn parent_for_thread<R: Runtime>(app: &AppHandle<R>, thread: &str) -> Result<Parent, String> {
    with_parent_admission(app, thread, |parent, _host| Ok(parent.clone()))
}
/// Canonical order matches import_local_sessions and checked pane binding:
/// runtime activity -> state.inner -> database.conn -> delegation journal.
/// No await or recursive getters inside the callback; leases cover the syscall.
pub(crate) fn with_parent_admission<R: Runtime, T>(
    app: &AppHandle<R>, thread: &str,
    f: impl FnOnce(&Parent, &dyn Fn(i64) -> Result<String, String>) -> Result<T, String>,
) -> Result<T, String> {
    crate::local_session_import::require_live_thread(thread)?;
    let state = app.state::<AppStateStore>();
    let db = app.state::<DatabaseStore>();
    state.with_canonical_snapshot(|snapshot| db.with_delegation_admission(thread, |record, event_id, host| {
        if record.imported_from.is_some() { return Err("Imported conversations are read-only".into()); }
        let workspace = snapshot.workspaces.iter().find(|w|w.workspace_id.0==record.workspace_id).ok_or("Parent workspace is closed")?;
        if workspace.imported_snapshot_only==Some(true) || workspace.attach_only { return Err("Parent workspace is read-only".into()); }
        let pane = workspace.surfaces.iter().find_map(|s|find_agent_chat_pane_id(&s.root,thread)).ok_or("Parent is no longer bound to an open pane")?;
        let (provider,bound) = workspace.surfaces.iter().find_map(|s|crate::state::agent_chat_thread_pair_for_pane(&s.root,&pane.0)).ok_or("Parent binding disappeared")?;
        if bound!=thread || serde_json::to_value(provider).map_err(|e|e.to_string())?.as_str()!=Some(&record.provider) { return Err("Parent database/pane/provider ownership mismatch".into()); }
        let parent=Parent {thread_id:thread.into(),provider,workspace_id:record.workspace_id,label:record.title.unwrap_or_else(||"Coding thread".into()),permission_mode:record.permission_mode.unwrap_or_default(),event_id};
        f(&parent,host)
    }))
}
fn validate_grant_at(parent: &Parent, grant: &Grant, host: &dyn Fn(i64) -> Result<String,String>) -> Result<(),String> {
    if !grant.enabled { return Err("Delegation grant is revoked".into()); }
    if host(grant.host_id)?!=grant.ssh_target { return Err("Host SSH identity changed; authorize the target again".into()); }
    if !permissions::permits(parent.provider,&parent.permission_mode,grant.provider,&grant.permission_mode) { return Err("Remote mode exceeds the parent's current permission ceiling (unknown/restricted modes fail closed)".into()); }
    Ok(())
}

fn host<R: Runtime>(app: &AppHandle<R>, id: i64) -> Result<crate::database::HostRecord, String> {
    app.state::<DatabaseStore>()
        .list_hosts()
        .into_iter()
        .find(|h| h.id == id)
        .ok_or("Configured SSH host is missing or deleted".into())
}
pub fn changed<R: Runtime>(app: &AppHandle<R>, parent: &str) {
    let _ = app.emit("delegation-changed", json!({"parent_thread_id":parent}));
}
/// Notify on durable visible changes, including partial mutations on errors.
/// Idle reads and idempotent retries do not invalidate their own readback.
pub(super) struct MutationNotice<R: Runtime> {
    app: AppHandle<R>, store: Arc<Journal>, parent: Parent, before: Value,
}
impl<R: Runtime> MutationNotice<R> {
    pub(super) fn for_task(app:&AppHandle<R>,task:&LocalTask)->Result<Self,String> {
        Self::new(app,&Parent {thread_id:task.parent_thread_id.clone(),workspace_id:task.parent_workspace_id.clone(),provider:task.parent_provider,label:String::new(),permission_mode:String::new(),event_id:None})
    }
    pub(super) fn new(app: &AppHandle<R>, parent: &Parent) -> Result<Self, String> {
        let store = journal(app)?;
        let before = Self::snapshot(&store, parent)?;
        Ok(Self { app: app.clone(), store, parent: parent.clone(), before })
    }
    fn snapshot(store: &Journal, parent: &Parent) -> Result<Value, String> {
        Ok(json!({"tasks":store.tasks()?.into_iter().filter(|t|t.parent_thread_id==parent.thread_id).collect::<Vec<_>>(),"grants":store.grants(&parent.workspace_id)?}))
    }
}
impl<R: Runtime> Drop for MutationNotice<R> {
    fn drop(&mut self) {
        if Self::snapshot(&self.store, &self.parent).is_ok_and(|after| after != self.before) {
            changed(&self.app, &self.parent.thread_id);
        }
    }
}
fn project_task<R:Runtime>(app:&AppHandle<R>,store:&Journal,parent:&Parent,mut task:LocalTask)->Result<LocalTask,String> {
    let outcome=store.delivery_outcome(&task.id)?;
    let reason=match outcome {
        DeliveryOutcome::Accepted=>Some(DeliveryReason::Accepted),
        DeliveryOutcome::Unknown=>Some(DeliveryReason::Unknown),
        DeliveryOutcome::InFlight=>Some(DeliveryReason::InFlight),
        _ if task.cancel_requested || task.status==crate::remote::tasks::TaskStatus::Cancelled || task.remote.as_ref().is_some_and(|r|r.cancel_requested)=>Some(DeliveryReason::Cancelled),
        _ if task.remote.is_none()=>Some(DeliveryReason::NoResult),
        _ if !task.status.is_terminal()=>Some(DeliveryReason::NotTerminal),
        _ if task.wake_state==WakeState::Delivered=>Some(DeliveryReason::AlreadyDelivered),
        _ if task.wake_state==WakeState::Delivering=>Some(DeliveryReason::InFlight),
        _ if store.tail_pending(&task.id)?=>Some(DeliveryReason::TailPending),
        _ if owned(store,parent,&task.id).and_then(|_|store.grant_for_task(&task.id)).and_then(|grant| {
            task.validate_target(&grant)?;
            if !permissions::permits(parent.provider,&store.ceiling(&task.id)?,grant.provider,&grant.permission_mode) {return Err("Original ceiling changed".into());}
            validate_grant(app,parent,&grant)
        }).is_err()=>Some(DeliveryReason::AuthorityUnavailable),
        _=>None,
    };
    task.delivery=DeliveryProjection {outcome,eligible:reason.is_none(),reason};
    task.responses.clear();
    if let Some(remote)=task.remote.as_ref() {
        let authority=owned(store,parent,&task.id).and_then(|_|store.grant_for_task(&task.id)).and_then(|grant| {
            task.validate_target(&grant)?;
            if !permissions::permits(parent.provider,&store.ceiling(&task.id)?,grant.provider,&grant.permission_mode) {return Err("Original ceiling changed".into());}
            validate_grant(app,parent,&grant)
        }).is_ok();
        for request in &remote.pending_requests {
            let (outcome,decision,binding)=store.response_outcome(&task.id,&request.request_id)?;
            let reason=match outcome {
                DeliveryOutcome::Accepted=>Some(ResponseReason::Accepted),
                DeliveryOutcome::Unknown=>Some(ResponseReason::Unknown),
                DeliveryOutcome::InFlight=>Some(ResponseReason::InFlight),
                _ if task.cancel_requested || remote.cancel_requested || task.status.is_terminal()=>Some(ResponseReason::Cancelled),
                _ if store.tail_pending(&task.id)?=>Some(ResponseReason::TailPending),
                _ if !authority=>Some(ResponseReason::AuthorityUnavailable),
                _ if outcome==DeliveryOutcome::BeforeSend && binding.as_ref()!=Some(request)=>Some(ResponseReason::PayloadChanged),
                _=>None,
            };
            task.responses.push(ResponseProjection {request_id:request.request_id.clone(),outcome,decision,eligible:reason.is_none(),reason});
        }
    }
    Ok(task)
}
fn value<T: serde::Serialize>(v: T) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}
fn string(args: &Value, name: &str) -> Result<String, String> {
    args.get(name)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("{name} is required"))
}
pub(super) fn owned(store: &Journal, parent: &Parent, id: &str) -> Result<LocalTask, String> {
    let t = store.task(id)?;
    if t.parent_thread_id != parent.thread_id
        || t.parent_workspace_id != parent.workspace_id
        || t.parent_provider != parent.provider
    {
        return Err("Task belongs to another parent thread".into());
    }
    Ok(t)
}
pub fn validate_grant<R: Runtime>(
    app: &AppHandle<R>,
    parent: &Parent,
    grant: &Grant,
) -> Result<(), String> {
    if !grant.enabled {
        return Err("Delegation grant is revoked".into());
    }
    if host(app, grant.host_id)?.ssh_target != grant.ssh_target {
        return Err("Host SSH identity changed; authorize the target again".into());
    }
    if !permissions::permits(
        parent.provider,
        &parent.permission_mode,
        grant.provider,
        &grant.permission_mode,
    ) {
        return Err("Remote mode exceeds the parent's current permission ceiling (unknown/restricted modes fail closed)".into());
    }
    Ok(())
}
async fn transport<R: Runtime>(
    app: &AppHandle<R>,
    target: &str,
    op: Operation,
    input: Option<Value>,
) -> Result<Value, String> {
    let transport: Arc<dyn TaskTransport> = app
        .try_state::<Arc<dyn TaskTransport>>()
        .map(|t| t.inner().clone())
        .or_else(|| {
            app.try_state::<super::coordinator::Coordinator>()
                .map(|c| c.transport.clone())
        })
        .unwrap_or_else(|| Arc::new(SshTransport));
    transport.call(target, op, input).await
}
pub async fn invoke<R: Runtime>(
    app: &AppHandle<R>,
    cmd: &str,
    args: Value,
) -> Result<Value, String> {
    if matches!(cmd, "delegation_respond" | "delegation_deliver") {
        if let Some(coordinator) = app.try_state::<super::coordinator::Coordinator>() {
            let handle = app.clone(); let command = cmd.to_owned();
            return coordinator.run_owned(async move { invoke_open(&handle, &command, args).await }).await;
        }
    }
    invoke_open(app, cmd, args).await
}
async fn invoke_open<R: Runtime>(app: &AppHandle<R>, cmd: &str, args: Value) -> Result<Value,String> {
    let store = journal(app)?;
    if cmd == "delegation_host_info" {
        let id = args
            .get("hostId")
            .and_then(Value::as_i64)
            .ok_or("hostId is required")?;
        let host = host(app, id)?;
        let info: crate::remote::tasks::TaskCapabilities = serde_json::from_value(
            transport(app, &host.ssh_target, Operation::Capabilities, None).await?,
        )
        .map_err(|e| e.to_string())?;
        if info.protocol_version != 1 {
            return Err("Host task protocol is incompatible".into());
        }
        store.cache_host(id, &info)?;
        return value(info);
    }
    let parent = parent_for_pane(app, &string(&args, "paneId")?)?;
    let _notice = if matches!(cmd,"delegation_list"|"delegation_read"|"delegation_grants") {None} else {Some(MutationNotice::new(app,&parent)?)};
    let result = match cmd {
        "delegation_grants" => value(store.grants(&parent.workspace_id)?),
        "delegation_list" => value(
            store
                .tasks()?
                .into_iter()
                .filter(|t| {
                    t.parent_thread_id == parent.thread_id
                        && t.parent_workspace_id == parent.workspace_id
                        && t.parent_provider == parent.provider
                })
                .collect::<Vec<_>>(),
        ),
        "delegation_authorize" => {
            let input: AuthorizeDelegationInput =
                serde_json::from_value(args.get("input").cloned().ok_or("input is required")?)
                    .map_err(|e| e.to_string())?;
            let host = host(app, input.host_id)?;
            let info: crate::remote::tasks::TaskCapabilities = serde_json::from_value(
                transport(app, &host.ssh_target, Operation::Capabilities, None).await?,
            )
            .map_err(|e| e.to_string())?;
            if info.protocol_version != 1 {
                return Err("Host task protocol is incompatible".into());
            }
            let workspace = info
                .workspaces
                .iter()
                .find(|w| w.path == input.workspace_path)
                .ok_or("Select an existing receiver-registered checkout")?;
            let caps = info
                .providers
                .iter()
                .find(|p| p.provider == input.provider)
                .and_then(|p| p.capabilities.as_ref())
                .ok_or("The receiver's installed/authenticated provider is unavailable")?;
            if !caps
                .permission_modes
                .iter()
                .any(|m| m.value == input.permission_mode)
            {
                return Err("Mode is not available on the receiver".into());
            }
            let prior = store.grants(&parent.workspace_id)?.into_iter().find(|g| {
                g.host_id == host.id
                    && g.ssh_target == host.ssh_target
                    && g.workspace_path == workspace.path
                    && g.provider == input.provider
                    && g.permission_mode == input.permission_mode
            });
            let grant = Grant {
                id: prior
                    .map(|g| g.id)
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                host_id: host.id,
                host_name: host.name,
                ssh_target: host.ssh_target,
                workspace_path: workspace.path.clone(),
                workspace_name: workspace.name.clone(),
                provider: input.provider,
                permission_mode: input.permission_mode,
                enabled: true,
                created_at: chrono::Utc::now().to_rfc3339(),
            };
            let current = parent_for_pane(app, &string(&args, "paneId")?)?;
            if current.thread_id != parent.thread_id
                || current.workspace_id != parent.workspace_id
                || current.provider != parent.provider
            {
                return Err("Parent pane ownership changed during authorization".into());
            }
            validate_grant(app, &current, &grant)?;
            store.cache_host(input.host_id, &info)?;
            store.put_grant(&parent.workspace_id, &grant)?;
            value(grant)
        }
        "delegate_task" => {
            let input: DelegateTaskInput =
                serde_json::from_value(args.get("input").cloned().ok_or("input is required")?)
                    .map_err(|e| e.to_string())?;
            admit(app, &parent, &input)
                .map(|t| {
                    start(app, &t.id);
                    t
                })
                .and_then(value)
        }
        "delegation_read" => {
            let id = string(&args, "taskId")?;
            owned(&store, &parent, &id)?;
            value(store.read(&id, args.get("cursor").and_then(Value::as_i64).unwrap_or(0))?)
        }
        "delegation_cancel" => {
            let id = string(&args, "taskId")?;
            owned(&store, &parent, &id)?;
            let task = store.cancel(&id)?;
            start(app, &id);
            value(task)
        }
        "delegation_revoke" => {
            let id = string(&args, "targetId")?;
            store.disable_grant(&parent.workspace_id, &id)?;
            for task in store
                .tasks()?
                .into_iter()
                .filter(|t| t.parent_workspace_id == parent.workspace_id)
            {
                if store.grant_for_task(&task.id).is_ok_and(|g| g.id == id) {
                    store.cancel(&task.id)?;
                    start(app, &task.id);
                    changed(app, &task.parent_thread_id);
                }
            }
            Ok(Value::Null)
        }
        "delegation_respond" => {
            let id = string(&args, "taskId")?;
            let task = owned(&store, &parent, &id)?;
            let grant = store.grant_for_task(&id)?;
            task.validate_target(&grant)?;
            validate_grant(app, &parent, &grant)?;
            if task.cancel_requested {
                return Err("Stop has already been requested".into());
            }
            let request_id = string(&args, "requestId")?;
            if !task.remote.as_ref().is_some_and(|t| {
                t.pending_requests
                    .iter()
                    .any(|r| r.request_id == request_id)
            }) {
                return Err("Approval is not currently pending for this task".into());
            }
            let decision: crate::agent_provider::ApprovalDecision = serde_json::from_value(
                args.get("decision")
                    .cloned()
                    .ok_or("decision is required")?,
            )
            .map_err(|e| e.to_string())?;
            let original:crate::remote::tasks::RemoteApproval=serde_json::from_value(args.get("originalRequest").cloned().ok_or("Original displayed approval consent identity is required")?).map_err(|e|e.to_string())?;
            if original.request_id!=request_id || !task.remote.as_ref().is_some_and(|t|t.pending_requests.iter().any(|r|r==&original)) {return Err("Displayed approval kind/payload changed before consent admission".into());}
            let immutable = crate::remote::tasks::RespondRequest {
                original_request: Some(original),
                request_id: request_id.clone(),
                decision,
            };
            if let Some(request)=task.remote.as_ref().and_then(|remote|remote.pending_requests.iter().find(|r|r.request_id==request_id && r.request_kind=="user-input")) {
                validate_question_decision(&request.payload,&immutable.decision)?;
            }
            let dispatch = store.claim_response(&id, &immutable)?;
            let channel: Arc<dyn TaskTransport> = app
                .try_state::<Arc<dyn TaskTransport>>()
                .map(|t| t.inner().clone())
                .or_else(|| {
                    app.try_state::<super::coordinator::Coordinator>()
                        .map(|c| c.transport.clone())
                })
                .unwrap_or_else(|| Arc::new(SshTransport));
            let response_cursor = if dispatch { 0 } else { store.follow_intent(&id)?.1 };
            let response = if dispatch {
                let result = channel
                    .call_guarded(
                        &task.target_ssh_target,
                        Operation::Respond { id: id.clone() },
                        Some(serde_json::to_value(&immutable).map_err(|e| e.to_string())?),
                        remote_dispatch_guard(app,&store,&task,Some(immutable.clone())),
                    )
                    .await;
                let result = match result {
                    Ok(data) => {
                        let acknowledgement = (|| {
                            let read: crate::remote::tasks::TaskRead =
                                serde_json::from_value(data.clone()).map_err(|e| e.to_string())?;
                            let expected = serde_json::to_value(&immutable.decision)
                                .map_err(|e| e.to_string())?;
                            if !read.approval_receipts.iter().any(|r| {
                                r.request_id == request_id
                                    && r.original_request == immutable.original_request
                                    && serde_json::to_value(&r.decision).ok().as_ref()
                                        == Some(&expected)
                            }) {
                                return Err(
                                    "Receiver did not acknowledge the immutable approval decision"
                                        .into(),
                                );
                            }
                            store.apply_remote(&id, &read)
                        })();
                        acknowledgement
                            .map(|()| data)
                            .map_err(super::transport::TransportError::Unknown)
                    }
                    Err(error) => Err(error),
                };
                store.finish_response(&id, &request_id, &result)?;
                result
            } else {
                // Ambiguous/in-flight attempts only read receiver-owned durable
                // receipts. Neither retries nor restart replay a native callback.
                channel
                    .call_outcome(
                        &task.target_ssh_target,
                        Operation::Read {
                            id: id.clone(),
                            cursor: response_cursor,
                        },
                        None,
                    )
                    .await
            };
            match response {
                Ok(data) => {
                    let read = serde_json::from_value(data).map_err(|e| e.to_string())?;
                    if !dispatch { store.apply_remote_after(&id, &read, response_cursor)?; }
                }
                Err(error) => {
                    store.update(&id, |t| {
                        t.connection_error =
                            Some(format!("Approval response is unconfirmed: {error}"))
                    })?;
                }
            }
            start(app, &id);
            value(store.task(&id)?)
        }
        "delegation_deliver" => {
            let id = string(&args, "taskId")?;
            let task = owned(&store, &parent, &id)?;
            if !project_task(app,&store,&parent,task.clone())?.delivery.eligible { return Err("This result is not available for explicit delivery".into()); }
            validate_grant(app, &parent, &store.grant_for_task(&id)?)?;
            if !task.status.is_terminal()
                || task.remote.is_none()
                || task.cancel_requested
                || matches!(
                    task.wake_state,
                    WakeState::Delivered | WakeState::Delivering
                )
            {
                return Err("This result is not available for explicit delivery".into());
            }
            store.explicit_delivery(&id)?;
            super::coordinator::wake_open(app, &id).await?;
            value(store.task(&id)?)
        }
        _ => Err("Unknown native delegation command".into()),
    };
    let output=result?;
    match cmd {
        "delegation_list"=>value(serde_json::from_value::<Vec<LocalTask>>(output).map_err(|e|e.to_string())?.into_iter().map(|task|project_task(app,&store,&parent,task)).collect::<Result<Vec<_>,_>>()?),
        "delegation_read"=>{
            let mut read:TaskRead=serde_json::from_value(output).map_err(|e|e.to_string())?;
            read.task=project_task(app,&store,&parent,read.task)?; value(read)
        }
        "delegate_task"|"delegation_cancel"|"delegation_respond"|"delegation_deliver"=>value(project_task(app,&store,&parent,serde_json::from_value(output).map_err(|e|e.to_string())?)?),
        _=>Ok(output),
    }
}

fn validate_question_decision(payload:&Value,decision:&crate::agent_provider::ApprovalDecision)->Result<(),String> {
    use crate::agent_provider::ApprovalDecision;
    if matches!(decision,ApprovalDecision::Deny{..}|ApprovalDecision::Cancel) {return Ok(());}
    let ApprovalDecision::Allow{updated_input:Some(updated),..}=decision else {return Err("Remote question requires structured answers, not bare Allow".into());};
    let original=payload.as_object().ok_or("Unsupported remote question payload")?;
    let output=updated.as_object().ok_or("Question answer input must be an object")?;
    if original.iter().filter(|(key,_)|key.as_str()!="answers").any(|(key,value)|output.get(key)!=Some(value)) || output.keys().any(|key|key!="answers" && !original.contains_key(key)) {return Err("Question answer must preserve the original question fields".into());}
    let questions=original.get("questions").and_then(Value::as_array).filter(|q|!q.is_empty()).ok_or("No supported remote questions")?;
    let answers=output.get("answers").and_then(Value::as_object).ok_or("Every remote question requires an answer")?;
    let mut seen=std::collections::HashSet::new();
    for question in questions {
        let title=question.get("question").and_then(Value::as_str).filter(|s|!s.is_empty()).ok_or("Unsupported remote question")?;
        if !seen.insert(title) || !answers.get(title).and_then(Value::as_str).is_some_and(|s|!s.trim().is_empty()) {return Err("Every remote question requires a unique nonempty answer".into());}
    }
    if answers.len()!=questions.len() {return Err("Remote answers do not match the questions".into());}
    Ok(())
}
struct RemoteDispatch<R:Runtime> {
    app:AppHandle<R>, store:Arc<Journal>, id:String, thread:String,
    response:Option<crate::remote::tasks::RespondRequest>,
    waker:Arc<futures_util::task::AtomicWaker>,
}
impl<R:Runtime> std::fmt::Debug for RemoteDispatch<R> {
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {f.debug_struct("RemoteDispatch").field("task",&self.id).finish()}
}
impl<R:Runtime> crate::json_rpc_child::dispatch::DispatchGuard for RemoteDispatch<R> {
    fn register(&self,waker:&std::task::Waker) {self.waker.register(waker);}
    fn poll_first(&self,write:&mut dyn FnMut()->crate::json_rpc_child::dispatch::WritePoll)->Result<crate::json_rpc_child::dispatch::WritePoll,String> {
        with_parent_admission(&self.app,&self.thread,|parent,host| {
            self.store.poll_transport(&self.id,self.response.as_ref(),|task,grant,ceiling| {
                if parent.thread_id!=task.parent_thread_id || parent.workspace_id!=task.parent_workspace_id || parent.provider!=task.parent_provider {return Err("Parent ownership changed at remote dispatch".into());}
                if !permissions::permits(parent.provider,ceiling,grant.provider,&grant.permission_mode) {return Err("Original permission ceiling denies remote dispatch".into());}
                validate_grant_at(parent,grant,host)
            },write)
        })
    }
}
pub(crate) fn remote_dispatch_guard<R:Runtime>(app:&AppHandle<R>,store:&Arc<Journal>,task:&LocalTask,response:Option<crate::remote::tasks::RespondRequest>)->Arc<dyn crate::json_rpc_child::dispatch::DispatchGuard> {
    let waker=Arc::new(futures_util::task::AtomicWaker::new());store.register_dispatch(&waker);
    Arc::new(RemoteDispatch {app:app.clone(),store:store.clone(),id:task.id.clone(),thread:task.parent_thread_id.clone(),response,waker})
}
