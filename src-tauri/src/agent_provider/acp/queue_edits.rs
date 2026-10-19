//! Editing and reordering an ACP session's follow-up queue; see
//! [`queue_edit`](crate::agent_provider::queue_edit). A selected item has
//! already left the queue, so only items still waiting can change.

use super::{AcpSession, QueuedTurn};
use crate::agent_provider::queue_edit::{self, QueueEntry};
use crate::agent_provider::{ProviderError, ProviderRuntimeEvent, SendTurnInput};

impl QueueEntry for QueuedTurn {
    fn queued_id(&self) -> &str {
        &self.id
    }
    fn input(&self) -> &SendTurnInput {
        &self.input
    }
    fn input_mut(&mut self) -> &mut SendTurnInput {
        &mut self.input
    }
}

impl AcpSession {
    pub async fn edit_queued(&self, queued_id: &str, text: &str) -> Result<bool, ProviderError> {
        // Published under the lock, so events keep the order of the edits.
        let mut state = self.state.lock().await;
        let edited = queue_edit::edit(&mut state.queued, &self.thread_id, queued_id, text)?;
        let Some(text) = edited else { return Ok(false) };
        let _ = self.event_tx.send(ProviderRuntimeEvent::QueuedTurnEdited {
            thread_id: self.thread_id.clone(),
            queued_id: queued_id.to_owned(),
            text,
        });
        Ok(true)
    }

    pub async fn move_queued(
        &self,
        queued_id: &str,
        before: Option<&str>,
    ) -> Result<bool, ProviderError> {
        let mut guard = self.state.lock().await;
        let state = &mut *guard;
        let pinned = state.queue_hold.pinned();
        let order = queue_edit::move_before(&mut state.queued, &self.thread_id, queued_id, before, pinned)?;
        let Some(queued_ids) = order else { return Ok(false) };
        let _ = self.event_tx.send(ProviderRuntimeEvent::QueuedTurnsReordered {
            thread_id: self.thread_id.clone(),
            queued_ids,
        });
        Ok(true)
    }
}
