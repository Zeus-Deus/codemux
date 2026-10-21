//! Steering: guidance pushed into a running Claude turn.
//!
//! The sidecar pushes the message into the live SDK prompt stream with
//! `priority: "next"`, so the CLI reads it when it builds its next API
//! request (usually once the current tool returns) without cancelling
//! anything. If the turn ends first, the CLI runs the message as a turn of
//! its own. Every `result` echoes the prompt uuids its turn consumed
//! (`user_message_uuids`), which tells the two cases apart: guidance still
//! pending after a result gets a continuation turn, so its output is
//! attributed and the follow-up queue cannot dispatch into the gap.
//!
//! A newer Stop always wins: guidance is refused into a turn being stopped,
//! and guidance the stopped turn never read dies with it (the sidecar ends
//! the CLI process rather than leave it queued there).
//!
//! Without the echo nothing can tell the two cases apart, so steering waits
//! until this CLI has been seen echoing prompt uuids.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::Value;

use super::{ClaudeSession, ClaudeSessionState};
use crate::agent_provider::claude::protocol::{SendTurnImage, SteerTurnParams, METHOD_STEER_TURN};
use crate::agent_provider::{
    ImageInput, ProviderError, ProviderRuntimeEvent, SendOutcome, SendTurnInput, SessionStatus,
    TurnId, TurnStartResult,
};
use crate::json_rpc_child::RpcChildError;

const AWAITING_USER: &str =
    "Claude is waiting on a question or approval. Answer it, or choose Queue or Interrupt and send.";
const SLASH_COMMAND: &str =
    "Slash commands can't steer a running turn. Choose Queue or Interrupt and send.";

/// Per-session steering state, kept under the session's state lock.
#[derive(Debug, Default)]
pub(crate) struct SteerState {
    /// Prompt uuids pushed into the running turn and not yet seen consumed.
    pending: Vec<String>,
    /// The turn an `interrupt` is ending.
    stopping: Option<TurnId>,
    /// The turn started for guidance its predecessor's result did not consume.
    continuation: Option<TurnId>,
    /// A continuation whose `Running` state has not been announced yet.
    unannounced: Option<TurnId>,
    /// The CLI reports consumed prompts (`user_message_uuids`).
    echoes: bool,
}

impl SteerState {
    /// An interrupt is ending `turn`; it takes no more guidance.
    pub fn stop(&mut self, turn: Option<TurnId>) {
        self.stopping = turn;
    }

    /// The interrupt was confirmed: guidance the turn never read died with it.
    pub fn stopped(&mut self) {
        self.drop_pending();
    }

    pub fn observe_echo(&mut self) {
        self.echoes = true;
    }

    /// The interrupt RPC failed, so the stopped work runs on. Returns whether
    /// guidance is still pending, which needs a continuation if the stopped
    /// turn already ended.
    pub fn stop_failed(&mut self, turn: &Option<TurnId>) -> bool {
        if &self.stopping == turn {
            self.stopping = None;
        }
        !self.pending.is_empty()
    }

    /// The SDK query ended or was rebuilt, losing any guidance it held.
    pub fn drop_pending(&mut self) {
        self.pending.clear();
        self.continuation = None;
        self.unannounced = None;
    }

    /// A top-level `result` ending `ended`. Returns whether guidance is still
    /// pending, which the CLI runs next as a turn of its own.
    fn settle_result(&mut self, result: &Value, ended: Option<&TurnId>) -> bool {
        let consumed = result.get("user_message_uuids").and_then(Value::as_array);
        if let Some(consumed) = consumed {
            self.pending
                .retain(|uuid| !consumed.iter().any(|c| c.as_str() == Some(uuid)));
        }
        self.continuation = None;
        self.unannounced = None;
        if ended.is_some() && ended == self.stopping.as_ref() {
            // The stopped turn's result. Its unread guidance dies with it once
            // the interrupt is confirmed; if that fails, `stop_failed` says so.
            return false;
        }
        if consumed.is_none() {
            // A CLI that does not echo cannot say. Assume consumed, which is
            // how such a CLI behaved before steering existed.
            self.pending.clear();
        }
        !self.pending.is_empty()
    }

    /// Guidance that never reached the CLI. Returns the continuation turn
    /// that was waiting only on it.
    fn forget(&mut self, uuid: &str) -> Option<TurnId> {
        self.pending.retain(|pending| pending != uuid);
        if self.pending.is_empty() {
            self.unannounced = None;
            self.continuation.take()
        } else {
            None
        }
    }
}

enum Plan {
    /// Deliver into `turn` as guidance tagged `uuid`.
    Steer { turn: TurnId, uuid: String },
    /// No turn to steer, or one a Stop is ending: send through the normal
    /// path, which starts it, or queues it ahead of a held queue.
    Send,
}

/// Decide under the state lock, recording the guidance as pending before
/// any result can race its delivery.
fn plan(state: &mut ClaudeSessionState, input: &SendTurnInput) -> Result<Plan, ProviderError> {
    let Some(turn) = state.active_turn.clone() else {
        if state.turn_busy() {
            return Err(ProviderError::ValidationError {
                message: AWAITING_USER.into(),
            });
        }
        return Ok(Plan::Send);
    };
    if state.queue_hold.is_held()
        || !state.steer.echoes
        || state.steer.stopping.as_ref() == Some(&turn)
        || input.dispatch_guard.is_some()
    {
        return Ok(Plan::Send);
    }
    if crate::agent_provider::slash_commands::leading_command(&input.text).is_some() {
        return Err(ProviderError::ValidationError {
            message: SLASH_COMMAND.into(),
        });
    }
    if let Some(checkpoint) = &input.turn_checkpoint {
        checkpoint.authorize_dispatch()?;
    }
    let uuid = uuid::Uuid::new_v4().to_string();
    state.steer.pending.push(uuid.clone());
    Ok(Plan::Steer { turn, uuid })
}

fn encode_images(images: &[ImageInput]) -> Vec<SendTurnImage> {
    images
        .iter()
        .map(|img| SendTurnImage {
            media_type: img.media_type.clone(),
            data_base64: base64::engine::general_purpose::STANDARD.encode(&img.data),
        })
        .collect()
}

impl ClaudeSession {
    /// Non-interrupting guidance for the running turn. Idle sessions, and
    /// turns a Stop is ending, take the normal send path instead.
    pub async fn steer_turn(
        self: &Arc<Self>,
        input: SendTurnInput,
    ) -> Result<TurnStartResult, ProviderError> {
        let plan = plan(&mut *self.state.lock().await, &input)?;
        match plan {
            Plan::Steer { turn, uuid } => {
                self.deliver_steer(&uuid, &input, |_| {}).await?;
                Ok(TurnStartResult {
                    steered: true,
                    turn_id: turn,
                    queued_id: None,
                })
            }
            Plan::Send => Ok(match self.enqueue_or_send(input).await? {
                SendOutcome::Started(turn_id) => TurnStartResult {
                    steered: false,
                    turn_id,
                    queued_id: None,
                },
                SendOutcome::Queued(queued_id) => TurnStartResult {
                    steered: false,
                    turn_id: TurnId(String::new()),
                    queued_id: Some(queued_id),
                },
            }),
        }
    }

    /// Steer with a queued message. It leaves the queue only once delivered;
    /// a failed delivery puts it back where it was. When it cannot steer, it
    /// is sent next instead, through a held queue too, without interrupting.
    pub async fn steer_queued(self: &Arc<Self>, queued_id: &str) -> Result<(), ProviderError> {
        let (turn, uuid, queued, index) = {
            let mut state = self.state.lock().await;
            let Some(index) = state
                .queued_turns
                .iter()
                .position(|q| q.queued_id == queued_id)
            else {
                return Ok(());
            };
            let queued = state.queued_turns.remove(index).expect("index in range");
            match plan(&mut state, &queued.input) {
                Ok(Plan::Steer { turn, uuid }) => (turn, uuid, queued, index),
                Ok(Plan::Send) => {
                    state.queued_turns.push_front(queued);
                    state.queue_hold.release_with(queued_id);
                    if index > 0 {
                        let order = crate::agent_provider::queue_edit::reordered(&state.queued_turns, &self.thread_id);
                        let _ = self.event_tx.send(order);
                    }
                    drop(state);
                    self.drain_queue().await;
                    return Ok(());
                }
                Err(error) => {
                    state.queued_turns.insert(index, queued);
                    return Err(error);
                }
            }
        };
        let input = queued.input.clone();
        let mut cancelled = None;
        let delivered = self
            .deliver_steer(&uuid, &input, |state| {
                if matches!(state.status, SessionStatus::Closed | SessionStatus::Error { .. }) {
                    // The session cancelled its queue while this was out of it.
                    cancelled = Some(queued.queued_id);
                } else {
                    let index = index.min(state.queued_turns.len());
                    state.queued_turns.insert(index, queued);
                }
            })
            .await;
        if let Some(queued_id) = cancelled {
            let _ = self.event_tx.send(ProviderRuntimeEvent::QueuedTurnCancelled {
                thread_id: self.thread_id.clone(),
                queued_id,
            });
        }
        delivered?;
        let text = input.display_text.unwrap_or(input.text);
        let _ = self.event_tx.send(ProviderRuntimeEvent::QueuedTurnDispatched {
            steered: true,
            thread_id: self.thread_id.clone(),
            queued_id: queued_id.to_owned(),
            turn_id: turn,
            text,
        });
        Ok(())
    }

    /// On failure `restore` runs under the same lock that settles the
    /// guidance, before the queue drains, so nothing overtakes what it puts
    /// back.
    async fn deliver_steer(
        self: &Arc<Self>,
        uuid: &str,
        input: &SendTurnInput,
        restore: impl FnOnce(&mut ClaudeSessionState),
    ) -> Result<(), ProviderError> {
        let params = SteerTurnParams {
            thread_id: self.thread_id.0.clone(),
            text: input.text.clone(),
            uuid: uuid.to_owned(),
            images: encode_images(&input.images),
        };
        let response = match input.turn_checkpoint.as_ref().map(|checkpoint| checkpoint.authorize_dispatch()).transpose() {
            Ok(_) => self.sidecar.request(METHOD_STEER_TURN, serde_json::to_value(&params).unwrap()).await,
            Err(error) => Err(RpcChildError::RpcError(crate::json_rpc_child::RpcError {code:-32001,message:error.to_string(),data:None})),
        };
        let Err(error) = response else {
            return Ok(());
        };
        {
            let mut state = self.state.lock().await;
            if let Some(turn) = state.steer.forget(uuid) {
                self.settle_orphaned_continuation(&mut state, &turn);
            }
            restore(&mut state);
        }
        // The turn may have ended while this was in flight.
        self.drain_queue().await;
        Err(match error {
            RpcChildError::RpcError(error) if error.code == -32601 => {
                ProviderError::ValidationError {
                    message: "This Claude sidecar can't steer. Choose Queue or Interrupt and send."
                        .into(),
                }
            }
            RpcChildError::RpcError(error) if error.message.contains("no running turn") => {
                ProviderError::ValidationError {
                    message: "The turn stopped before your guidance arrived, so it was not sent."
                        .into(),
                }
            }
            other => ProviderError::RpcError {
                message: format!("Guidance was not delivered: {other}"),
            },
        })
    }

    /// A continuation started for guidance that then failed to deliver will
    /// never see a result. Return the session to idle.
    fn settle_orphaned_continuation(&self, state: &mut ClaudeSessionState, turn: &TurnId) {
        if state.active_turn.as_ref() != Some(turn) {
            return;
        }
        self.native_authority.finish(&turn.0);
        state.active_turn = None;
        if matches!(state.status, SessionStatus::Running { .. }) {
            state.status = SessionStatus::Ready;
            let _ = self
                .event_tx
                .send(ProviderRuntimeEvent::SessionStateChanged {
                    thread_id: self.thread_id.clone(),
                    status: SessionStatus::Ready,
                });
        }
    }

    /// Called with the turn's `result`, after it cleared the active turn.
    /// Guidance the turn did not consume runs next as the CLI's own turn;
    /// mark it running now so nothing dispatches into the gap.
    pub(super) fn continue_unconsumed_steers(
        &self,
        state: &mut ClaudeSessionState,
        result: &Value,
        ended: Option<TurnId>,
    ) {
        if state.steer.settle_result(result, ended.as_ref()) {
            self.begin_continuation(state);
        }
    }

    /// The interrupt failed: guidance the stopped turn left unread will run
    /// as the CLI's next turn, if that turn already ended.
    pub(super) fn recover_failed_stop(&self, state: &mut ClaudeSessionState, stopped: &Option<TurnId>) {
        if state.steer.stop_failed(stopped) && state.active_turn.is_none() {
            self.begin_continuation(state);
        }
    }

    fn begin_continuation(&self, state: &mut ClaudeSessionState) {
        if !matches!(state.status, SessionStatus::Ready) {
            return;
        }
        let turn = TurnId(format!("claude-turn-{}", uuid::Uuid::new_v4()));
        let Some(generation) = self.native_authority.begin_continuation() else { return; };
        self.native_authority.bind(generation, &turn.0);
        state.active_turn = Some(turn.clone());
        state.status = SessionStatus::Running {
            active_turn: turn.clone(),
        };
        state.steer.continuation = Some(turn.clone());
        state.steer.unannounced = Some(turn);
    }

    /// Announce a continuation once the finished turn's events are out, so
    /// subscribers see the completion before the next turn starts.
    pub(super) async fn announce_steer_continuation(&self) {
        let mut state = self.state.lock().await;
        let Some(turn) = state.steer.unannounced.take() else {
            return;
        };
        if state.active_turn.as_ref() != Some(&turn) {
            return;
        }
        let _ = self
            .event_tx
            .send(ProviderRuntimeEvent::SessionStateChanged {
                thread_id: self.thread_id.clone(),
                status: SessionStatus::Running { active_turn: turn },
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn steer_with(pending: &[&str]) -> SteerState {
        SteerState {
            pending: pending.iter().map(|p| p.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn result_that_consumed_the_guidance_settles_it() {
        let mut steer = steer_with(&["s1"]);
        assert!(!steer.settle_result(&json!({"type": "result", "user_message_uuids": ["p", "s1"]}), None));
        assert!(steer.pending.is_empty());
    }

    #[test]
    fn guidance_missing_from_the_result_stays_pending() {
        let mut steer = steer_with(&["s1", "s2"]);
        assert!(steer.settle_result(&json!({"type": "result", "user_message_uuids": ["p", "s1"]}), None));
        assert_eq!(steer.pending, ["s2"]);
    }

    #[test]
    fn result_without_an_echo_assumes_consumed() {
        let mut steer = steer_with(&["s1"]);
        assert!(!steer.settle_result(&json!({"type": "result"}), None));
        assert!(steer.pending.is_empty());
    }

    #[test]
    fn the_stopped_turns_result_waits_for_the_stop_outcome() {
        let turn = TurnId("t".into());
        let mut steer = steer_with(&["s1"]);
        steer.stop(Some(turn.clone()));
        assert!(!steer.settle_result(&json!({"user_message_uuids": ["p"]}), Some(&turn)));
        // A failed stop then reports the guidance as still to run.
        assert!(steer.stop_failed(&Some(turn)));
        assert_eq!(steer.pending, ["s1"]);
    }

    #[test]
    fn a_failed_stop_does_not_revive_guidance_the_result_consumed() {
        let turn = TurnId("t".into());
        let mut steer = steer_with(&["s1"]);
        steer.stop(Some(turn.clone()));
        assert!(!steer.settle_result(&json!({"user_message_uuids": ["p", "s1"]}), Some(&turn)));
        assert!(!steer.stop_failed(&Some(turn)));
    }

    #[test]
    fn a_failed_stop_keeps_pending_guidance() {
        let turn = TurnId("t".into());
        let mut steer = steer_with(&["s1"]);
        steer.stop(Some(turn.clone()));
        assert!(steer.stop_failed(&Some(turn.clone())));
        assert!(steer.stopping.is_none());
        // The turn ran on and finished without reading the guidance.
        assert!(steer.settle_result(&json!({"user_message_uuids": ["p"]}), Some(&turn)));
        assert_eq!(steer.pending, ["s1"]);
    }

    #[test]
    fn a_confirmed_stop_drops_pending_guidance() {
        let mut steer = steer_with(&["s1"]);
        steer.continuation = Some(TurnId("c".into()));
        steer.stop(Some(TurnId("c".into())));
        steer.stopped();
        assert!(steer.pending.is_empty());
        assert!(steer.continuation.is_none());
    }

    #[test]
    fn forgetting_the_last_guidance_orphans_its_continuation() {
        let mut steer = steer_with(&["s1", "s2"]);
        steer.continuation = Some(TurnId("c".into()));
        assert_eq!(steer.forget("s1"), None);
        assert_eq!(steer.forget("s2"), Some(TurnId("c".into())));
        assert!(steer.continuation.is_none());
    }
}
