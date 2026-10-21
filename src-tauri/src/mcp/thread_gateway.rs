//! Session-owned authority for transports that cannot carry an in-process caller.
use super::registry::HostCaller;
use crate::{
    agent_provider::{ProviderError, ProviderKind, TurnDispatchCheckpoint},
    delegation::{app::NativeActor, authority::NativeAuthority},
};
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub(crate) struct GatewaySession {
    id: String,
    thread: String,
    provider: ProviderKind,
    workspace: Option<String>,
    session: Mutex<String>,
    mode: Mutex<Option<String>>,
    pub(crate) authority: Arc<NativeAuthority>,
}
impl PartialEq for GatewaySession {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for GatewaySession {}
impl GatewaySession {
    pub(crate) fn new(
        thread: &str,
        provider: ProviderKind,
        workspace: Option<String>,
        mode: Option<String>,
    ) -> Arc<Self> {
        Arc::new(Self {
            id: uuid::Uuid::new_v4().to_string(),
            thread: thread.into(),
            provider,
            workspace,
            session: Mutex::new(String::new()),
            authority: NativeAuthority::with_policy(thread, provider, mode.as_deref()),
            mode: Mutex::new(mode),
        })
    }
    pub(crate) fn bind_session(&self, session: &str) {
        *self.session.lock().unwrap() = session.into();
    }
    pub(crate) fn set_mode(&self, mode: Option<String>) {
        self.authority.set_policy(self.provider, mode.as_deref());
        *self.mode.lock().unwrap() = mode;
    }
    pub(crate) fn begin(
        &self,
        turn: &str,
        checkpoint: Option<&Arc<dyn TurnDispatchCheckpoint>>,
    ) -> Result<(), ProviderError> {
        let stop = checkpoint
            .map(|c| c.native_stop_generation())
            .transpose()?
            .unwrap_or_default();
        let generation = self.authority.begin(stop);
        self.authority.bind(generation, turn);
        Ok(())
    }
    pub(crate) fn caller(&self) -> HostCaller {
        HostCaller {
            control: None,
            thread_id: self.thread.clone(),
            provider: self.provider,
            workspace_id: self.workspace.clone(),
            permission_mode: self.mode.lock().unwrap().clone(),
        }
    }
    pub(crate) fn actor(&self) -> NativeActor {
        NativeActor {
            thread_id: self.thread.clone(),
            provider: self.provider,
            workspace_id: self.workspace.clone(),
            session_id: self.session.lock().unwrap().clone(),
            permission_mode: self.mode.lock().unwrap().clone(),
            permit: self.authority.capture(),
        }
    }
}
impl Drop for GatewaySession {
    fn drop(&mut self) {
        self.authority.revoke();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GatewayBinding {
    pub(crate) session: Arc<GatewaySession>,
    pub(crate) epoch: u64,
}
impl GatewayBinding {
    pub(crate) fn new(session: Arc<GatewaySession>) -> Self {
        Self {
            epoch: session.authority.token_epoch(),
            session,
        }
    }
    pub(crate) fn current(&self) -> bool {
        self.session.authority.session_live() && self.epoch == self.session.authority.token_epoch()
    }
}
