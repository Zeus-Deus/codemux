//! In-process authority: conversation ids are routing, never capabilities.
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex, MutexGuard, Weak},
};
use tokio::task::AbortHandle;

static RUNTIMES: LazyLock<Mutex<HashMap<String, Weak<NativeAuthority>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug)]
pub(crate) struct NativeAuthority {
    session_key: String,
    state: Mutex<State>,
    changed: tokio::sync::Notify,
}
#[derive(Debug)]
pub(crate) struct State {
    live: bool,
    tools_live: bool,
    generation: u64,
    turn: Option<String>,
    callbacks: Vec<AbortHandle>,
    activity: u64,
    pending: bool,
    dispatch_wakers: Vec<Weak<futures_util::task::AtomicWaker>>,
}
#[derive(Debug, Clone)]
pub(crate) struct NativePermit {
    owner: Arc<NativeAuthority>,
    generation: u64,
    turn: Option<String>,
    pending: bool,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AuthorityIdentity {
    session_key: String,
    generation: u64,
    activity: u64,
    pub run_id: Option<String>,
}
impl AuthorityIdentity {
    pub fn verify(&self,thread:&str)->Result<(),String>{self.admit(thread,||Ok(()))}
    /// Revocation and synchronous admission share a fence. No provider wait
    /// or async work is allowed while this closure owns the caller permit.
    pub fn admit<T>(&self,thread:&str,work:impl FnOnce()->Result<T,String>)->Result<T,String>{
        let owner=RUNTIMES.lock().unwrap().get(thread).and_then(Weak::upgrade).ok_or("stale_caller: native session is unavailable")?;
        let state=owner.state.lock().unwrap();
        if owner.session_key!=self.session_key || !state.live || !state.tools_live || state.generation!=self.generation
            || state.activity!=self.activity || state.turn.is_none() || state.turn!=self.run_id {return Err("stale_caller: native session or run authority changed".into());}
        work()
    }
}
impl NativeAuthority {
    pub fn new(thread: &str) -> Arc<Self> {
        let owner = Arc::new(Self {
            session_key: uuid::Uuid::new_v4().to_string(),
            changed: tokio::sync::Notify::new(),
            state: Mutex::new(State {
                live: true,
                tools_live: false,
                generation: 0,
                turn: None,
                callbacks: Vec::new(),
                activity: 0,
                pending: false,
                dispatch_wakers: Vec::new(),
            }),
        });
        let mut runtimes = RUNTIMES.lock().unwrap();
        runtimes.retain(|_, v| v.strong_count() > 0);
        if let Some(old) = runtimes
            .insert(thread.into(), Arc::downgrade(&owner))
            .and_then(|v| v.upgrade())
        {
            old.revoke();
        }
        owner
    }
    // Synchronous capability revocation precedes every asynchronous Stop wait.
    pub fn user_activity_thread(thread: &str, stop: bool) {
        if let Some(owner) = RUNTIMES.lock().unwrap().get(thread).and_then(Weak::upgrade) {
            if stop { owner.invalidate(); } else { owner.human_activity(); }
        }
    }
    pub fn begin(&self) -> u64 {
        let mut state = self.state.lock().unwrap();
        state.generation += 1;
        state.tools_live = true;
        state.turn = None;
        state.pending = true;
        self.changed.notify_waiters();
        state.generation
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
    pub fn finish(&self, turn: &str) {
        let mut state = self.state.lock().unwrap();
        if state.turn.as_deref() == Some(turn) {
            state.generation += 1;
            state.turn = None;
            state.pending = false;
            self.changed.notify_waiters();
        }
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
    pub fn track(&self, job: AbortHandle) {
        let mut state = self.state.lock().unwrap();
        state.callbacks.retain(|job| !job.is_finished());
        if state.live && state.tools_live {
            state.callbacks.push(job);
        } else {
            job.abort();
        }
    }
    #[cfg(test)]
    pub fn current_permit(thread: &str) -> NativePermit {
        RUNTIMES
            .lock()
            .unwrap()
            .get(thread)
            .and_then(Weak::upgrade)
            .unwrap()
            .capture()
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
        let owner = RUNTIMES
            .lock()
            .unwrap()
            .get(thread)
            .and_then(Weak::upgrade)
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
        Ok(AuthorityIdentity { session_key: self.owner.session_key.clone(), generation: self.generation, activity: self.activity, run_id: state.turn.clone() })
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

    #[test]
    fn stop_keeps_pending_and_acknowledged_lifecycle_but_revokes_tools() {
        let owner = NativeAuthority::new("authority-overlapping-send");
        let generation = owner.begin();
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
            let generation = owner.begin();
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
            let later = owner.begin();
            assert!(owner.bind(later, "second"));
            assert!(early.wait_ready().await.is_err());
            assert!(early.admit().is_err());
        }
    }

    #[tokio::test]
    async fn pending_callback_wakes_and_rejects_stop_completion_and_runtime_loss() {
        for ending in ["stop", "complete", "revoke"] {
            let owner = NativeAuthority::new(&format!("authority-end-{ending}"));
            let generation = owner.begin();
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
