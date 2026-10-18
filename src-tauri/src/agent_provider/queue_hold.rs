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
}

impl QueueHold {
    /// A user Stop: hold the queue, dropping any pending release.
    pub fn hold(&mut self) {
        self.held = true;
        self.release = None;
    }

    pub fn is_held(&self) -> bool {
        self.held
    }

    /// Whether a drain may dispatch the queue's `front` item. An empty queue
    /// has nothing to hold, so the hold clears.
    pub fn admits(&mut self, front: Option<&str>) -> bool {
        if !self.held {
            return true;
        }
        match front {
            None => {
                self.held = false;
                self.release = None;
                false
            }
            Some(id) => self.release.as_deref() == Some(id),
        }
    }

    /// An explicit send of queued item `id`: let it through a held queue.
    pub fn release_with(&mut self, id: &str) {
        if self.held {
            self.release = Some(id.to_owned());
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

    /// A message sent directly by hand: lift the hold, returning whether it
    /// was held so a failed send can [`restore`](Self::restore) it. A pending
    /// release survives for that restore; only a newer [`hold`](Self::hold)
    /// drops it, and it is ignored while the queue is not held.
    pub fn take(&mut self) -> bool {
        std::mem::take(&mut self.held)
    }

    /// Undo [`take`](Self::take) after a send that did not start a turn.
    pub fn restore(&mut self, was_held: bool) {
        if was_held {
            self.held = true;
        }
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
    fn empty_queue_clears_the_hold() {
        let mut hold = QueueHold::default();
        hold.hold();
        assert!(!hold.admits(None));
        assert!(!hold.is_held());
    }

    #[test]
    fn failed_manual_send_restores_the_hold() {
        let mut hold = QueueHold::default();
        hold.hold();
        let was_held = hold.take();
        assert!(!hold.is_held());
        hold.restore(was_held);
        assert!(hold.is_held());
        let mut unheld = QueueHold::default();
        let was_held = unheld.take();
        unheld.restore(was_held);
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
        hold.restore(was_held);
        assert!(hold.admits(Some("m")));
    }

    #[test]
    fn stop_during_a_taken_hold_drops_the_release() {
        let mut hold = QueueHold::default();
        hold.hold();
        hold.release_with("m");
        let was_held = hold.take();
        hold.hold();
        hold.restore(was_held);
        assert!(!hold.admits(Some("m")));
    }
}
