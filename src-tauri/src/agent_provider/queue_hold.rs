//! Follow-up queue hold after a user Stop.
//!
//! Pressing Stop must not immediately dispatch the next queued message: the
//! user may have stopped because the run was going wrong. A held queue stays
//! put until the user explicitly sends something, and resumes behind that
//! send once it has started. Normal turn completion drains as before.

/// Per-session hold state, kept under the session's state lock.
#[derive(Debug, Default)]
pub(crate) struct QueueHold {
    held: bool,
    /// The queued item an explicit send promoted; it may dispatch through a
    /// held queue, and the hold lifts once it starts.
    release: Option<String>,
    /// A manual send has temporarily lifted the hold via [`take`](Self::take).
    /// Releases recorded meanwhile survive its [`restore`](Self::restore).
    taken: bool,
}

impl QueueHold {
    /// A user Stop: hold the queue, dropping any pending release.
    pub fn hold(&mut self) {
        self.held = true;
        self.taken = false;
        self.release = None;
    }

    pub fn is_held(&self) -> bool {
        self.held
    }

    /// The item an explicit send let through the hold, which must stay at
    /// the front of the queue to dispatch.
    pub fn pinned(&self) -> Option<&str> {
        self.release.as_deref().filter(|_| self.held || self.taken)
    }

    /// Whether a message sent by hand now goes ahead of the queue: a Stop
    /// holds it, or another manual send has the hold taken and may restore it.
    pub fn goes_first(&self) -> bool {
        self.held || self.taken
    }

    /// Whether a drain may dispatch the queue's `front` item. An empty queue
    /// retains the hold until an explicit human release.
    pub fn admits(&mut self, front: Option<&str>) -> bool {
        if !self.held {
            return true;
        }
        match front {
            None => false,
            Some(id) => self.release.as_deref() == Some(id),
        }
    }

    /// Shared drain policy for every adapter. Stop retains the queue, a
    /// refused item stays visible without blocking later work, and disposed
    /// wakes can be retired even while the queue is held.
    pub fn can_select(&mut self, id: &str, state: super::types::QueuedDeliveryState) -> bool {
        use super::types::QueuedDeliveryState::*;
        match state {
            Held => { self.hold(); false }
            Blocked => false,
            Disposed => true,
            Ready => self.admits(Some(id)),
        }
    }

    /// An explicit send of queued item `id`: let it through a held queue.
    /// Returns the release it replaced, for [`revert_release`](Self::revert_release).
    pub fn release_with(&mut self, id: &str) -> Option<String> {
        if self.held || self.taken {
            self.release.replace(id.to_owned())
        } else {
            None
        }
    }

    /// The send of `id` failed before dispatch: give the release back to the
    /// item it replaced, unless a Stop or another send has moved on since.
    pub fn revert_release(&mut self, id: &str, previous: Option<String>) {
        if self.release.as_deref() == Some(id) {
            self.release = previous;
        }
    }

    /// The dispatch of `id` finished. The hold lifts only if it started.
    pub fn settle(&mut self, id: &str, started: bool) {
        if self.release.as_deref() == Some(id) {
            self.release = None;
            if started {
                self.held = false;
            }
        }
    }

    /// A message sent directly by hand: lift the hold while it is in flight,
    /// returning whether it was held. Settle it with
    /// [`settle_take`](Self::settle_take). A pending release survives the
    /// take; only a newer [`hold`](Self::hold) drops it.
    pub fn take(&mut self) -> bool {
        let was_held = std::mem::take(&mut self.held);
        if was_held {
            self.taken = true;
        }
        was_held
    }

    /// Settle a [`take`](Self::take): a send that started a turn lifts the
    /// hold for good; one that did not restores it, with any release recorded
    /// meanwhile. Returns false when there was nothing to settle, including
    /// when a newer Stop has already held the queue again.
    pub fn settle_take(&mut self, was_held: bool, started: bool) -> bool {
        if !was_held || !self.taken {
            return false;
        }
        self.taken = false;
        if started {
            self.release = None;
        } else {
            self.held = true;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unheld_queue_always_drains() {
        let mut hold = QueueHold::default();
        assert!(hold.admits(Some("a")));
        assert!(hold.admits(None));
    }

    #[test]
    fn held_queue_admits_only_the_released_item_until_it_starts() {
        let mut hold = QueueHold::default();
        hold.hold();
        assert!(!hold.admits(Some("a")));
        hold.release_with("b");
        assert!(!hold.admits(Some("a")));
        assert!(hold.admits(Some("b")));
        // A failed dispatch keeps the rest of the queue held.
        hold.settle("b", false);
        assert!(!hold.admits(Some("a")));
        hold.release_with("a");
        hold.settle("a", true);
        assert!(hold.admits(Some("c")));
    }

    #[test]
    fn stop_during_a_release_wins() {
        let mut hold = QueueHold::default();
        hold.hold();
        hold.release_with("a");
        hold.hold();
        hold.settle("a", true);
        assert!(hold.is_held());
    }

    #[test]
    fn orchestration_message_empty_queue_retains_stop_hold() {
        let mut hold = QueueHold::default();
        hold.hold();
        assert!(!hold.admits(None));
        assert!(hold.is_held());
    }

    #[test]
    fn failed_manual_send_restores_the_hold() {
        let mut hold = QueueHold::default();
        hold.hold();
        let was_held = hold.take();
        assert!(!hold.is_held());
        hold.settle_take(was_held, false);
        assert!(hold.is_held());
        let mut unheld = QueueHold::default();
        let was_held = unheld.take();
        unheld.settle_take(was_held, false);
        assert!(!unheld.is_held());
    }

    #[test]
    fn restored_hold_keeps_an_earlier_release() {
        // A message sent while the stopped turn settles goes first; a steer
        // that does not start a turn must not strand it.
        let mut hold = QueueHold::default();
        hold.hold();
        hold.release_with("m");
        let was_held = hold.take();
        hold.settle_take(was_held, false);
        assert!(hold.admits(Some("m")));
    }

    #[test]
    fn failed_send_now_returns_the_release_it_replaced() {
        let mut hold = QueueHold::default();
        hold.hold();
        hold.release_with("m");
        let previous = hold.release_with("q");
        hold.revert_release("q", previous);
        assert!(hold.admits(Some("m")));
        // A Stop in between wins.
        let previous = hold.release_with("q");
        hold.hold();
        hold.revert_release("q", previous);
        assert!(!hold.admits(Some("m")));
    }

    #[test]
    fn send_now_during_a_taken_hold_survives_its_restore() {
        // A manual send lifted the hold; "send now" on Q interrupts it before
        // its prompt, and the manual send then restores the hold.
        let mut hold = QueueHold::default();
        hold.hold();
        let was_held = hold.take();
        hold.release_with("q");
        hold.settle_take(was_held, false);
        assert!(hold.admits(Some("q")));
        // A Stop in between still wins.
        let mut hold = QueueHold::default();
        hold.hold();
        let was_held = hold.take();
        hold.release_with("q");
        hold.hold();
        hold.settle_take(was_held, false);
        assert!(!hold.admits(Some("q")));
    }

    #[test]
    fn manual_send_during_a_taken_hold_goes_first_until_the_take_settles() {
        let mut hold = QueueHold::default();
        hold.hold();
        let was_held = hold.take();
        // A second manual message N while M is in flight goes ahead.
        assert!(hold.goes_first());
        hold.release_with("n");
        // M fails before its prompt: N is let through the restored hold.
        assert!(hold.settle_take(was_held, false));
        assert!(hold.admits(Some("n")));
        // Had M started instead, the take ends and later sends are ordinary.
        let mut hold = QueueHold::default();
        hold.hold();
        let was_held = hold.take();
        assert!(hold.settle_take(was_held, true));
        assert!(!hold.goes_first());
        assert!(!hold.is_held());
    }

    #[test]
    fn orchestration_message_drain_policy_skips_refused_items_without_releasing_stop() {
        use crate::agent_provider::types::QueuedDeliveryState::*;
        let mut hold=QueueHold::default();
        assert!(!hold.can_select("revoked",Blocked));
        assert!(hold.can_select("next",Ready));
        hold.hold();
        assert!(!hold.can_select("revoked",Blocked));
        assert!(!hold.can_select("next",Ready));
        assert!(hold.can_select("disposed",Disposed));
        hold.release_with("human");
        assert!(hold.can_select("human",Ready));
        hold.settle("human",false);
        assert!(!hold.can_select("next",Ready));
        hold.release_with("human-2");
        hold.settle("human-2",true);
        assert!(hold.can_select("next",Ready));
    }

    #[test]
    fn stop_during_a_taken_hold_drops_the_release() {
        let mut hold = QueueHold::default();
        hold.hold();
        hold.release_with("m");
        let was_held = hold.take();
        hold.hold();
        hold.settle_take(was_held, false);
        assert!(!hold.admits(Some("m")));
    }
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct TestCheckpoint {
    pub release: bool,
    pub held: std::sync::atomic::AtomicBool,
}
#[cfg(test)]
#[async_trait::async_trait]
impl crate::agent_provider::TurnDispatchCheckpoint for TestCheckpoint {
    fn releases_queue_hold(&self) -> bool { self.release }
    fn queued_delivery_state(&self) -> crate::agent_provider::types::QueuedDeliveryState {
        if self.held.load(std::sync::atomic::Ordering::SeqCst) { crate::agent_provider::types::QueuedDeliveryState::Held }
        else { crate::agent_provider::types::QueuedDeliveryState::Ready }
    }
    async fn prepare(&self) {}
    async fn commit(&self) {}
    async fn abort(&self) {}
}
