//! Dispatch-time authority for the existing native session owner.
use super::{
    service::{authorize, permission_ceiling, resolve_thread, Target},
    ControlCaller,
};
use crate::agent_provider::{ProviderKind, StartSessionInput};
use crate::state::{AppStateStore, PaneNodeSnapshot};
use tauri::{AppHandle, Manager, Runtime};

#[derive(Clone)]
pub struct NativeControlGuard {
    pub caller: ControlCaller,
    pub workspace_id: String,
    pub thread_id: String,
    pub provider: ProviderKind,
    pub(crate) admission: Option<(std::sync::Arc<super::state::ThreadGate>, u64)>,
}
impl NativeControlGuard {
    pub(crate) fn retain_admission<R: Runtime>(mut self, app: &AppHandle<R>) -> Self {
        if self.admission.is_some() { return self; }
        let gate = app.state::<super::NativeControlState>().thread_gate(&self.thread_id);
        let generation = gate.generation();
        self.admission = Some((gate, generation));
        self
    }
    pub fn verify<R: Runtime>(&self, app: &AppHandle<R>) -> Result<(), String> {
        if let Some((gate, generation)) = &self.admission { gate.verify(*generation)?; }
        // This is the parent's authority choke point. Until the connector is
        // adopted it denies every outside grant; afterwards it validates the
        // current connector config/grant rather than an admission snapshot.
        authorize(app, &self.caller).map_err(|e| e.to_string())?;
        let target = Target {
            workspace_id: self.workspace_id.clone(),
            thread_id: self.thread_id.clone(),
        };
        let (record, provider) = resolve_thread(app, &target, true).map_err(|e| e.to_string())?;
        if provider != self.provider {
            return Err("stale_thread_binding: provider changed after admission".into());
        }
        permission_ceiling(&self.caller, provider, record.permission_mode.as_deref())
            .map_err(|e| e.to_string())?;
        if self.caller.access == super::ControlAccess::Supervised {
            if let Some(state) = app.try_state::<super::NativeControlState>() {
                state.verify_worker_mode(
                    &self.thread_id,
                    provider,
                    record.permission_mode.as_deref(),
                )?;
            }
        }
        let cwd = record
            .cwd
            .as_deref()
            .filter(|cwd| !cwd.is_empty())
            .ok_or_else(|| {
                "invalid_cwd: native control requires a stored working directory".to_string()
            })?;
        let stored = std::path::Path::new(cwd);
        if !stored.is_absolute() || !stored.is_dir() {
            return Err(
                "invalid_cwd: stored working directory is not an existing absolute directory"
                    .into(),
            );
        }
        let snapshot = app.state::<AppStateStore>().snapshot();
        let workspace = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id.0 == self.workspace_id)
            .ok_or_else(|| "workspace_not_found".to_string())?;
        let pane_cwd = workspace
            .surfaces
            .iter()
            .find_map(|s| binding_cwd(&s.root, &self.thread_id, self.provider))
            .ok_or_else(|| "stale_thread_binding: no matching pane".to_string())?;
        let expected = std::path::Path::new(pane_cwd.unwrap_or(&workspace.cwd));
        let same = stored
            .canonicalize()
            .ok()
            .zip(expected.canonicalize().ok())
            .is_some_and(|(a, b)| a == b);
        if !same {
            return Err(
                "stale_thread_binding: persisted and pane working directories differ".into(),
            );
        }
        Ok(())
    }
    pub(crate) fn verify_target(&self, provider: ProviderKind, thread: &str) -> Result<(), String> {
        if self.provider != provider || self.thread_id != thread {
            Err("stale_thread_binding: guarded target differs from command".into())
        } else {
            Ok(())
        }
    }
    pub(crate) fn verify_start<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        input: &StartSessionInput,
    ) -> Result<(), String> {
        self.verify_target(self.provider, &input.thread_id.0)?;
        self.verify(app)?;
        permission_ceiling(
            &self.caller,
            self.provider,
            input.permission_mode.as_deref(),
        )
        .map_err(|e| e.to_string())?;
        let record = app
            .state::<crate::database::DatabaseStore>()
            .get_agent_chat_session(&self.thread_id)
            .ok_or_else(|| "thread_not_found".to_string())?;
        if record.cwd.as_deref().map(std::path::Path::new) != Some(input.cwd.as_path())
            || record.permission_mode != input.permission_mode
            || record.model != input.model
            || record.effort != input.effort
            || record.context_window != input.context_window
            || record.fast_mode != input.fast_mode
        {
            return Err("stale_session_configuration: guarded launch no longer matches persisted configuration".into());
        }
        Ok(())
    }
    pub(crate) fn verify_mode(&self, mode: Option<&str>) -> Result<(), String> {
        permission_ceiling(&self.caller, self.provider, mode).map_err(|e| e.to_string())
    }
}
pub(crate) struct NativeStartCancellation {
    pub(crate) gate: std::sync::Arc<super::state::ThreadGate>,
    pub(crate) generation: u64,
}
#[async_trait::async_trait]
impl crate::agent_provider::types::SessionStartCancellation for NativeStartCancellation {
    fn check(&self) -> Result<(), String> { self.gate.verify(self.generation) }
    async fn cancelled(&self) { self.gate.cancelled_since(self.generation).await; }
}

pub(crate) struct GuardedTurnCheckpoint<R: Runtime> {
    pub(crate) app: AppHandle<R>,
    pub(crate) control: NativeControlGuard,
    pub(crate) inner: Option<std::sync::Arc<dyn crate::agent_provider::types::TurnDispatchCheckpoint>>,
}
impl<R: Runtime> std::fmt::Debug for GuardedTurnCheckpoint<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("GuardedTurnCheckpoint") }
}
#[async_trait::async_trait]
impl<R: Runtime> crate::agent_provider::types::TurnDispatchCheckpoint for GuardedTurnCheckpoint<R> {
    fn authorize_dispatch(&self) -> Result<(), crate::agent_provider::ProviderError> {
        self.control.verify(&self.app).map_err(|message| crate::agent_provider::ProviderError::ValidationError {message})?;
        if let Some(inner) = &self.inner { inner.authorize_dispatch()?; }
        Ok(())
    }
    async fn prepare(&self) { if let Some(inner) = &self.inner { inner.prepare().await; } }
    async fn commit(&self) { if let Some(inner) = &self.inner { inner.commit().await; } }
    async fn abort(&self) { if let Some(inner) = &self.inner { inner.abort().await; } }
}

fn binding_cwd<'a>(
    pane: &'a PaneNodeSnapshot,
    thread: &str,
    provider: ProviderKind,
) -> Option<Option<&'a str>> {
    match pane {
        PaneNodeSnapshot::AgentChat {
            thread_id: Some(bound),
            provider: Some(kind),
            cwd,
            ..
        } if bound == thread && *kind == provider => Some(cwd.as_deref()),
        PaneNodeSnapshot::Split { children, .. } => children
            .iter()
            .find_map(|p| binding_cwd(p, thread, provider)),
        _ => None,
    }
}
