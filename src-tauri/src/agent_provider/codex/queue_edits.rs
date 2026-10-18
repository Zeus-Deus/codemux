//! Editing and reordering Codex's follow-up queue; see
//! [`queue_edit`](crate::agent_provider::queue_edit).

use std::sync::Arc;

use super::{CodexSession, QueuedTurn};
use crate::agent_provider::queue_edit::{self, QueueEntry};
use crate::agent_provider::{ProviderError, ProviderRuntimeEvent, SendTurnInput};

impl QueueEntry for QueuedTurn {
    fn queued_id(&self) -> &str {
        &self.queued_id
    }
    fn input(&self) -> &SendTurnInput {
        &self.input
    }
    fn input_mut(&mut self) -> &mut SendTurnInput {
        &mut self.input
    }
}

impl CodexSession {
    /// Holds `outbound` like a steer does: a steered item stays queued until
    /// it is delivered, and an edit must not change it on the way. A drain
    /// skipped meanwhile runs once the lock is released.
    pub async fn edit_queued(self: &Arc<Self>, queued_id: &str, text: &str) -> Result<bool, ProviderError> {
        let outbound = self.outbound.lock().await;
        let edited = {
            let mut state = self.state.lock().await;
            queue_edit::edit(&mut state.queued_turns, &self.thread_id, queued_id, text)
        };
        if let Ok(Some(text)) = &edited {
            let _ = self.event_tx().send(ProviderRuntimeEvent::QueuedTurnEdited {
                thread_id: self.thread_id.clone(),
                queued_id: queued_id.to_owned(),
                text: text.clone(),
            });
        }
        drop(outbound);
        self.drain_queue().await;
        edited.map(|text| text.is_some())
    }

    pub async fn move_queued(
        self: &Arc<Self>,
        queued_id: &str,
        before: Option<&str>,
    ) -> Result<bool, ProviderError> {
        let outbound = self.outbound.lock().await;
        let order = {
            let mut state = self.state.lock().await;
            let state = &mut *state;
            let pinned = state.queue_hold.pinned();
            queue_edit::move_before(&mut state.queued_turns, &self.thread_id, queued_id, before, pinned)
        };
        if let Ok(Some(queued_ids)) = &order {
            let _ = self.event_tx().send(ProviderRuntimeEvent::QueuedTurnsReordered {
                thread_id: self.thread_id.clone(),
                queued_ids: queued_ids.clone(),
            });
        }
        drop(outbound);
        self.drain_queue().await;
        order.map(|order| order.is_some())
    }
}
