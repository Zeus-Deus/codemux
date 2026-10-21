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
    /// The agent has reached the gateway with this session's credential.
    contacted: std::sync::atomic::AtomicBool,
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
            contacted: Default::default(),
            authority: NativeAuthority::with_policy(thread, provider, mode.as_deref()),
            mode: Mutex::new(mode),
        })
    }
    pub(crate) fn bind_session(&self, session: &str) {
        *self.session.lock().unwrap() = session.into();
    }
    pub(crate) fn session_id(&self) -> String {
        self.session.lock().unwrap().clone()
    }
    pub(crate) fn mark_contacted(&self) {
        self.contacted
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    pub(crate) fn contacted(&self) -> bool {
        self.contacted.load(std::sync::atomic::Ordering::Relaxed)
    }
    /// A changed mode is a policy transition: callbacks admitted under the
    /// old reach end, later ones run under the new one. Stop is separate.
    pub(crate) fn set_mode(&self, mode: Option<String>) {
        let changed = *self.mode.lock().unwrap() != mode;
        let _transition = changed.then(|| self.authority.policy_transition());
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

/// A gateway credential acts for one app session. Stop leaves it valid:
/// the authority's turn fence already refuses stopped-turn calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GatewayBinding {
    pub(crate) session: Arc<GatewaySession>,
}
impl GatewayBinding {
    pub(crate) fn new(session: Arc<GatewaySession>) -> Self {
        Self { session }
    }
    pub(crate) fn current(&self) -> bool {
        self.session.authority.session_live()
    }
}

#[cfg(test)]
mod policy_transition_tests {
    use super::*;
    use crate::agent_control::permissions::Ceiling;

    #[tokio::test]
    async fn a_mode_change_ends_callbacks_admitted_under_the_old_policy() {
        let thread = "gateway-mode-transition";
        let session = GatewaySession::new(thread, ProviderKind::Cursor, None, Some("agent".into()));
        session.begin("turn", None).unwrap();
        let old = session.authority.capture();
        let job = tokio::spawn(std::future::pending::<()>());
        session.authority.track(&old, job.abort_handle());
        // A replayed notification of the current mode changes nothing.
        session.set_mode(Some("agent".into()));
        old.wait_ready().await.unwrap();
        session.set_mode(Some("ask".into()));
        let ended = tokio::time::timeout(std::time::Duration::from_secs(5), job).await;
        assert!(ended.expect("old callback kept running").unwrap_err().is_cancelled());
        assert!(old.wait_ready().await.is_err());
        session.authority.capture().wait_ready().await.unwrap();
        assert_eq!(
            NativeAuthority::effective_ceiling(thread).unwrap(),
            Ceiling::for_mode(ProviderKind::Cursor, Some("ask"))
        );
    }
}
