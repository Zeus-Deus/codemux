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
    state: Mutex<State>,
}
#[derive(Debug)]
pub(crate) struct State {
    live: bool,
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
}
impl NativeAuthority {
    pub fn new(thread: &str) -> Arc<Self> {
        let owner = Arc::new(Self {
            state: Mutex::new(State {
                live: true,
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
    // A synchronous generation change precedes every asynchronous Stop wait.
    pub fn invalidate_thread(thread: &str) {
        if let Some(owner) = RUNTIMES.lock().unwrap().get(thread).and_then(Weak::upgrade) {
            owner.invalidate();
        }
    }
    pub fn begin(&self) -> u64 {
        let mut state = self.state.lock().unwrap();
        state.generation += 1;
        state.turn = None;
        state.pending = true;
        state.generation
    }
    pub fn bind(&self, generation: u64, turn: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.live && state.generation == generation {
            state.turn = Some(turn.into());
            state.pending = false;
            return true;
        }
        false
    }
    /// Parent turn/started can arrive before its correlated RPC response.
    pub fn observe_start(&self, turn: &str) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.live && state.pending && state.turn.is_none() {
            state.turn = Some(turn.into());
        }
        state.live && state.turn.as_deref() == Some(turn)
    }
    pub fn finish_pending(&self, generation: u64) {
        let mut state = self.state.lock().unwrap();
        if state.generation == generation {
            state.generation += 1;
            state.turn = None;
            state.pending = false;
        }
    }
    pub fn finish(&self, turn: &str) {
        let mut state = self.state.lock().unwrap();
        if state.turn.as_deref() == Some(turn) {
            state.generation += 1;
            state.turn = None;
            state.pending = false;
        }
    }
    pub fn invalidate(&self) {
        let mut state = self.state.lock().unwrap();
        state.activity += 1;
        state.dispatch_wakers.retain(|w| w.strong_count() > 0);
        for waker in state.dispatch_wakers.iter().filter_map(Weak::upgrade) {
            waker.wake();
        }
        state.pending = false;
        state.generation += 1;
        state.turn = None;
    }
    pub fn revoke(&self) {
        let mut state = self.state.lock().unwrap();
        state.activity += 1;
        state.dispatch_wakers.retain(|w| w.strong_count() > 0);
        for waker in state.dispatch_wakers.iter().filter_map(Weak::upgrade) {
            waker.wake();
        }
        state.pending = false;
        state.live = false;
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
        }
    }
    pub fn track(&self, job: AbortHandle) {
        let mut state = self.state.lock().unwrap();
        state.callbacks.retain(|job| !job.is_finished());
        if state.live {
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
    /// Hold across final canonical validation and synchronous journal mutation.
    /// No await is permitted while this guard is held.
    pub fn admit(&self) -> Result<MutexGuard<'_, State>, String> {
        let state = self.owner.state.lock().unwrap();
        if !state.live
            || self.turn.is_none()
            || self.generation != state.generation
            || self.turn != state.turn
        {
            return Err("Native caller runtime/turn authority was revoked".into());
        }
        Ok(state)
    }
}
