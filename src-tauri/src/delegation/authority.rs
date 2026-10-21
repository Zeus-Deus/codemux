//! In-process authority: conversation ids are routing, never capabilities.
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex, MutexGuard, Weak},
};
use tokio::task::AbortHandle;

// Lock order: the registry only resolves/replaces weak runtime references. Drop
// its guard before taking NativeAuthority::state or invoking any authority method.
// Authority admission may take delegation state / journal locks for synchronous
// durable mutations; release it before recursive revocation or provider work.
static RUNTIMES: LazyLock<Mutex<HashMap<String, Weak<NativeAuthority>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn runtime(thread: &str) -> Option<Arc<NativeAuthority>> {
    let owner = RUNTIMES.lock().unwrap().get(thread).and_then(Weak::upgrade);
    #[cfg(test)]
    after_runtime_lookup();
    owner
}

#[cfg(test)]
fn after_runtime_lookup() {
    LOOKUP_HOOK.with(|hook| {
        let hook = hook.borrow_mut().take();
        if let Some(hook) = hook { hook(); }
    });
}

#[cfg(test)]
thread_local! {
    static LOOKUP_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = Default::default();
}

#[cfg(test)]
pub(crate) fn on_next_runtime_lookup(hook: impl FnOnce() + 'static) {
    LOOKUP_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

#[derive(Debug)]
pub(crate) struct NativeAuthority {
    session_key: String,
    state: Mutex<State>,
    changed: tokio::sync::Notify,
}
#[derive(Debug)]
pub(crate) struct State {
    live: bool,
    policy: Option<crate::agent_control::permissions::Ceiling>,
    tools_live: bool,
    policy_epoch: u64,
    policy_changes: u32,
    generation: u64,
    turn: Option<String>,
    stop_generation: u64,
    callbacks: Vec<AbortHandle>,
    activity: u64,
    pending: bool,
    dispatch_wakers: Vec<Weak<futures_util::task::AtomicWaker>>,
}
#[derive(Debug, Clone)]
pub(crate) struct NativePermit {
    owner: Arc<NativeAuthority>,
    generation: u64,
    policy_epoch: u64,
    turn: Option<String>,
    pending: bool,
}
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AuthorityIdentity {
    session_key: String,
    generation: u64,
    pub run_id: Option<String>,
    #[serde(default)]
    policy_epoch: u64,
    #[serde(default)]
    pub stop_generation: u64,
}
impl AuthorityIdentity {
    pub async fn wait_revoked(
        &self,
        thread: &str,
        ceiling: crate::agent_control::permissions::Ceiling,
    ) {
        let owner = runtime(thread);
        let Some(owner) = owner else {
            return;
        };
        loop {
            let changed = owner.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.verify(thread).is_err() {
                return;
            }
            if owner
                .state
                .lock()
                .unwrap()
                .policy
                .is_none_or(|current| !current.permits(ceiling))
            {
                return;
            }
            changed.await;
        }
    }
    pub fn verify(&self, thread: &str) -> Result<(), String> {
        self.admit(thread, || Ok(()))
    }
    /// Revocation and synchronous durable admission share a fence. No recursive
    /// authority calls, provider waits or async work may run inside this closure.
    pub fn admit<T>(
        &self,
        thread: &str,
        work: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let owner = runtime(thread)
            .ok_or("stale_caller: native session is unavailable")?;
        let state = owner.state.lock().unwrap();
        if owner.session_key != self.session_key
            || !state.live
            || !state.tools_live
            || state.policy_changes > 0
            || state.policy_epoch != self.policy_epoch
            || state.generation != self.generation
            || state.turn.is_none()
            || state.turn != self.run_id
        {
            return Err("stale_caller: native session or run authority changed".into());
        }
        work()
    }
}
impl NativeAuthority {
    pub fn with_policy(
        thread: &str,
        provider: crate::agent_provider::ProviderKind,
        mode: Option<&str>,
    ) -> Arc<Self> {
        let owner = Self::new(thread);
        owner.set_policy(provider, mode);
        owner
    }
    pub fn set_policy(&self, provider: crate::agent_provider::ProviderKind, mode: Option<&str>) {
        self.state.lock().unwrap().policy =
            crate::agent_control::permissions::Ceiling::for_mode(provider, mode);
        self.changed.notify_waiters();
    }
    /// Revoke callbacks across an in-place policy RPC without revoking the
    /// turn lifecycle. Stop keeps its separate, one-way tools_live fence.
    pub fn policy_transition(&self) -> PolicyTransition<'_> {
        let mut state = self.state.lock().unwrap();
        state.policy_epoch += 1;
        state.policy_changes += 1;
        for job in state.callbacks.drain(..) { job.abort(); }
        self.changed.notify_waiters();
        PolicyTransition { owner: self }
    }
    pub(crate) fn set_ceiling(&self, ceiling: crate::agent_control::permissions::Ceiling) {
        self.state.lock().unwrap().policy = Some(ceiling);
        self.changed.notify_waiters();
    }
    pub fn effective_ceiling(
        thread: &str,
    ) -> Result<Option<crate::agent_control::permissions::Ceiling>, String> {
        let owner = runtime(thread);
        let Some(owner) = owner else {
            return Ok(None);
        };
        let state = owner.state.lock().unwrap();
        state
            .policy
            .map(Some)
            .ok_or_else(|| "unsupported_permission_ceiling: current native mode is unknown".into())
    }
    pub fn new(thread: &str) -> Arc<Self> {
        let owner = Arc::new(Self {
            session_key: uuid::Uuid::new_v4().to_string(),
            changed: tokio::sync::Notify::new(),
            state: Mutex::new(State {
                live: true,
                policy: None,
                tools_live: false,
                policy_epoch: 0,
                policy_changes: 0,
                generation: 0,
                turn: None,
                stop_generation: 0,
                callbacks: Vec::new(),
                activity: 0,
                pending: false,
                dispatch_wakers: Vec::new(),
            }),
        });
        let old = {
            let mut runtimes = RUNTIMES.lock().unwrap();
            runtimes.retain(|_, v| v.strong_count() > 0);
            runtimes
                .insert(thread.into(), Arc::downgrade(&owner))
                .and_then(|v| v.upgrade())
        };
        #[cfg(test)]
        after_runtime_lookup();
        if let Some(old) = old {
            old.revoke();
        }
        owner
    }
    // Synchronous capability revocation precedes every asynchronous Stop wait.
    pub fn user_activity_thread(thread: &str, stop: bool) {
        let owner = runtime(thread);
        if let Some(owner) = owner {
            if stop { owner.invalidate(); } else { owner.human_activity(); }
        }
    }
    pub fn begin(&self, stop_generation: u64) -> u64 {
        let mut state = self.state.lock().unwrap();
        state.generation += 1;
        state.tools_live = true;
        state.stop_generation = stop_generation;
        state.turn = None;
        state.pending = true;
        self.changed.notify_waiters();
        state.generation
    }
    /// A failed Stop can leave already-delivered guidance running. Track that
    /// lifecycle with the original Stop fence without re-arming native tools.
    pub fn begin_continuation(&self) -> Option<u64> {
        let mut state = self.state.lock().unwrap();
        if !state.live { return None; }
        state.generation += 1;
        state.turn = None;
        state.pending = true;
        self.changed.notify_waiters();
        Some(state.generation)
    }
    pub fn bind(&self, generation: u64, turn: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.live && state.generation == generation {
            state.turn = Some(turn.into());
            state.pending = false;
            self.changed.notify_waiters();
            return true;
        }
        false
    }
    /// Parent turn/started can arrive before its correlated RPC response.
    pub fn observe_start(&self, turn: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.live && state.pending && state.turn.is_none() {
            state.turn = Some(turn.into());
            self.changed.notify_waiters();
        }
        state.live && state.turn.as_deref() == Some(turn)
    }
    pub fn finish_pending(&self, generation: u64) {
        let mut state = self.state.lock().unwrap();
        if state.generation == generation {
            state.generation += 1;
            state.turn = None;
            state.pending = false;
            self.changed.notify_waiters();
        }
    }
    pub(crate) fn finish_thread(thread: &str, turn: &str) {
        let owner = RUNTIMES.lock().unwrap().get(thread).and_then(Weak::upgrade);
        if let Some(owner) = owner {
            owner.finish(turn);
        }
    }
    pub fn finish(&self, turn: &str) {
        let mut state = self.state.lock().unwrap();
        if state.turn.as_deref() == Some(turn) {
            state.generation += 1;
            state.turn = None;
            state.pending = false;
            self.changed.notify_waiters();
        }
    }
    pub(crate) fn session_live(&self) -> bool {
        self.state.lock().unwrap().live
    }
    pub fn invalidate(&self) {
        let mut state = self.state.lock().unwrap();
        state.activity += 1;
        state.tools_live = false;
        for job in state.callbacks.drain(..) { job.abort(); }
        state.dispatch_wakers.retain(|w| w.strong_count() > 0);
        for waker in state.dispatch_wakers.iter().filter_map(Weak::upgrade) {
            waker.wake();
        }
        // Stop revokes tools, but the pending acknowledgement still owns its
        // turn ID so Queue and Stop can settle it without reviving tools.
        self.changed.notify_waiters();
    }
    fn human_activity(&self) {
        let mut state = self.state.lock().unwrap();
        // A human send supersedes an automatic result dispatch, while the
        // current turn (and any tool request it is waiting for) stays live.
        state.activity += 1;
        state.dispatch_wakers.retain(|w| w.strong_count() > 0);
        for waker in state.dispatch_wakers.iter().filter_map(Weak::upgrade) { waker.wake(); }
    }
    pub fn finish_current(&self) {
        let mut state = self.state.lock().unwrap();
        state.generation += 1;
        state.turn = None;
        state.pending = false;
        self.changed.notify_waiters();
    }
    pub fn revoke(&self) {
        let mut state = self.state.lock().unwrap();
        state.activity += 1;
        state.tools_live = false;
        state.dispatch_wakers.retain(|w| w.strong_count() > 0);
        for waker in state.dispatch_wakers.iter().filter_map(Weak::upgrade) {
            waker.wake();
        }
        state.pending = false;
        state.live = false;
        self.changed.notify_waiters();
        state.generation += 1;
        state.turn = None;
        for job in state.callbacks.drain(..) {
            job.abort();
        }
    }
    pub fn capture(self: &Arc<Self>) -> NativePermit {
        let state = self.state.lock().unwrap();
        NativePermit {
            owner: self.clone(),
            generation: state.generation,
            policy_epoch: state.policy_epoch,
            turn: state.turn.clone(),
            pending: state.pending,
        }
    }
    /// Keep the request's explicit turn identity even when its notification
    /// consumer has not yet processed the correlated start acknowledgement.
    pub fn capture_for_turn(self: &Arc<Self>, turn: &str) -> NativePermit {
        let mut permit = self.capture();
        permit.turn = Some(turn.into());
        permit
    }
    pub fn track(&self, permit: &NativePermit, job: AbortHandle) {
        let mut state = self.state.lock().unwrap();
        state.callbacks.retain(|job| !job.is_finished());
        if state.live && state.tools_live && state.policy_changes == 0
            && std::ptr::eq(self, permit.owner.as_ref())
            && state.generation == permit.generation
            && state.policy_epoch == permit.policy_epoch
        {
            state.callbacks.push(job);
        } else {
            job.abort();
        }
    }
    #[cfg(test)]
    pub fn current_permit(thread: &str) -> NativePermit {
        runtime(thread).unwrap().capture()
    }
}
pub(crate) struct PolicyTransition<'a> { owner: &'a NativeAuthority }
impl Drop for PolicyTransition<'_> {
    fn drop(&mut self) {
        let mut state = self.owner.state.lock().unwrap();
        // Requests captured while the policy was unresolved cannot revive.
        state.policy_epoch += 1;
        state.policy_changes -= 1;
        self.owner.changed.notify_waiters();
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DispatchPermit {
    owner: Arc<NativeAuthority>,
    activity: u64,
    pub(crate) waker: Arc<futures_util::task::AtomicWaker>,
}
impl DispatchPermit {
    pub fn capture(thread: &str) -> Result<Self, String> {
        let owner = runtime(thread)
            .ok_or("No qualified native parent runtime")?;
        let waker = Arc::new(futures_util::task::AtomicWaker::new());
        let activity = {
            let mut state = owner.state.lock().unwrap();
            state.dispatch_wakers.retain(|w| w.strong_count() > 0);
            state.dispatch_wakers.push(Arc::downgrade(&waker));
            state.activity
        };
        Ok(Self {
            owner,
            activity,
            waker,
        })
    }
    pub fn admit(&self) -> Result<MutexGuard<'_, State>, String> {
        let state = self.owner.state.lock().unwrap();
        if !state.live || state.activity != self.activity {
            return Err("Parent activity superseded result dispatch".into());
        }
        Ok(state)
    }
}
impl NativePermit {
    pub fn identity(&self) -> Result<AuthorityIdentity, String> {
        let state = self.admit()?;
        Ok(AuthorityIdentity {
            session_key: self.owner.session_key.clone(),
            generation: self.generation,
            policy_epoch: self.policy_epoch,
            run_id: state.turn.clone(),
            stop_generation: state.stop_generation,
        })
    }

    pub async fn wait_ready(&self) -> Result<(), String> {
        loop {
            let changed = self.owner.changed.notified();
            // Register before inspecting state, avoiding a missed bind/Stop.
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let state = self.owner.state.lock().unwrap();
                if !state.live
                    || !state.tools_live
                    || state.policy_changes > 0
                    || self.policy_epoch != state.policy_epoch
                    || self.generation != state.generation
                {
                    return Err("Native caller runtime/turn authority was revoked".into());
                }
                if let Some(turn) = &state.turn {
                    if self.turn.as_ref().is_none_or(|expected| expected == turn) {
                        return Ok(());
                    }
                    return Err("Native callback belongs to another turn".into());
                }
                if !self.pending || !state.pending {
                    return Err("Native callback has no pending turn".into());
                }
            }
            changed.await;
        }
    }

    /// Hold across final canonical validation and synchronous journal mutation.
    /// No await is permitted while this guard is held.
    pub fn admit(&self) -> Result<MutexGuard<'_, State>, String> {
        let state = self.owner.state.lock().unwrap();
        if !state.live
            || !state.tools_live
            || state.policy_changes > 0
            || self.policy_epoch != state.policy_epoch
            || state.turn.is_none()
            || self.generation != state.generation
            || match &self.turn {
                Some(turn) => state.turn.as_ref() != Some(turn),
                None => !self.pending,
            }
        {
            return Err("Native caller runtime/turn authority was revoked".into());
        }
        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn review600_late_callback_registration_cannot_cross_policy_transition() {
        let owner = NativeAuthority::new("review600-policy-track");
        let generation = owner.begin(4);
        assert!(owner.bind(generation, "turn"));
        let old = owner.capture();
        old.wait_ready().await.unwrap();
        let transition = owner.policy_transition();
        drop(transition);
        // The task passed wait_ready before the RPC, but registration lagged it.
        let job = tokio::spawn(std::future::pending::<()>());
        owner.track(&old, job.abort_handle());
        assert!(tokio::time::timeout(std::time::Duration::from_secs(1), job).await.unwrap().unwrap_err().is_cancelled());
        let new = owner.capture();
        let job = tokio::spawn(async { "new callback" });
        owner.track(&new, job.abort_handle());
        assert_eq!(job.await.unwrap(), "new callback");
    }

    #[tokio::test]
    async fn review600_policy_transition_never_rearms_stopped_callbacks() {
        for stopped_before in [false, true] {
            let thread = format!("review600-policy-stop-{stopped_before}");
            let owner = NativeAuthority::with_policy(&thread, crate::agent_provider::ProviderKind::Claude, Some("default"));
            let generation = owner.begin(4);
            assert!(owner.bind(generation, "turn"));
            let old = owner.capture();
            if stopped_before { owner.invalidate(); }
            let transition = owner.policy_transition();
            let during = owner.capture();
            assert!(old.wait_ready().await.is_err());
            assert!(during.wait_ready().await.is_err());
            if !stopped_before { owner.invalidate(); }
            owner.set_policy(crate::agent_provider::ProviderKind::Claude, Some("bypassPermissions"));
            drop(transition);
            assert!(owner.capture().wait_ready().await.is_err());
            assert!(during.wait_ready().await.is_err());
            // The in-flight lifecycle acknowledgement remains valid.
            assert!(owner.bind(generation, "turn"));
        }
    }

    #[tokio::test]
    async fn review600_failed_or_cancelled_policy_transition_restores_old_policy_only() {
        let thread = "review600-policy-failure";
        let owner = NativeAuthority::with_policy(thread, crate::agent_provider::ProviderKind::Claude, Some("default"));
        let generation = owner.begin(4);
        assert!(owner.bind(generation, "turn"));
        let old = owner.capture();
        let transition = owner.policy_transition();
        let during = owner.capture();
        drop(transition); // RPC failed/cancelled: no applied policy to publish.
        assert!(old.wait_ready().await.is_err());
        assert!(during.wait_ready().await.is_err());
        owner.capture().wait_ready().await.unwrap();
        assert_eq!(NativeAuthority::effective_ceiling(thread).unwrap(), crate::agent_control::permissions::Ceiling::for_mode(crate::agent_provider::ProviderKind::Claude, Some("default")));
    }

    #[test]
    fn native_stop_generation_stays_with_turn_and_steer_continuations() {
        let owner = NativeAuthority::new("authority-stop-generation");
        let generation = owner.begin(7);
        assert!(owner.bind(generation,"first"));
        assert_eq!(owner.capture().identity().unwrap().stop_generation,7);
        owner.finish("first");
        let continuation = owner.begin_continuation().unwrap();
        assert!(owner.bind(continuation,"continuation"));
        assert_eq!(owner.capture().identity().unwrap().stop_generation,7);
        owner.invalidate();
        let stopped_continuation = owner.begin_continuation().unwrap();
        assert!(owner.bind(stopped_continuation,"after-failed-stop"));
        assert!(owner.capture().identity().is_err());
        owner.revoke();
        assert!(owner.begin_continuation().is_none());
    }

    #[test]
    fn stop_keeps_pending_and_acknowledged_lifecycle_but_revokes_tools() {
        let owner = NativeAuthority::new("authority-overlapping-send");
        let generation = owner.begin(0);
        let early = owner.capture();
        owner.invalidate();
        assert!(owner.observe_start("first-turn"));
        assert!(
            owner.bind(generation, "first-turn"),
            "ordinary acknowledgement must survive Queue/Stop intent"
        );
        assert!(early.admit().is_err());
        assert!(
            owner.capture().admit().is_err(),
            "callbacks arriving after Stop cannot reacquire authority"
        );
        owner.finish("first-turn");
        assert!(
            !owner.bind(generation, "first-turn"),
            "actual completion must prevent revival"
        );
    }

    #[tokio::test]
    async fn early_callback_waits_for_its_original_ack_and_never_uses_a_later_turn() {
        for explicit in [false, true] {
            let owner = NativeAuthority::new(&format!("authority-early-{explicit}"));
            let generation = owner.begin(0);
            let early = if explicit {
                owner.capture_for_turn("first")
            } else {
                owner.capture()
            };
            assert!(early.admit().is_err());
            let waiter = early.clone();
            let job = tokio::spawn(async move { waiter.wait_ready().await });
            tokio::task::yield_now().await;
            assert!(!job.is_finished());
            assert!(owner.bind(generation, "first"));
            job.await.unwrap().unwrap();
            assert!(early.admit().is_ok());
            assert!(owner
                .capture_for_turn("foreign")
                .wait_ready()
                .await
                .is_err());
            owner.finish("first");
            let later = owner.begin(0);
            assert!(owner.bind(later, "second"));
            assert!(early.wait_ready().await.is_err());
            assert!(early.admit().is_err());
        }
    }

    #[tokio::test]
    async fn pending_callback_wakes_and_rejects_stop_completion_and_runtime_loss() {
        for ending in ["stop", "complete", "revoke"] {
            let owner = NativeAuthority::new(&format!("authority-end-{ending}"));
            let generation = owner.begin(0);
            let pending = owner.capture();
            let job = tokio::spawn(async move { pending.wait_ready().await });
            tokio::task::yield_now().await;
            match ending {
                "stop" => owner.invalidate(),
                "complete" => owner.finish_pending(generation),
                _ => owner.revoke(),
            }
            assert!(tokio::time::timeout(std::time::Duration::from_secs(1), job)
                .await
                .unwrap()
                .unwrap()
                .is_err());
        }
    }
}
