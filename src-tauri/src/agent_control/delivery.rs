//! Persisted message delivery. Native session/queue ownership stays in agent_chat.
use super::{ControlCaller, NativeControlGuard, NativeControlState};
use crate::{
    agent_provider::{message::*, ProviderKind, TurnDispatchCheckpoint},
    commands::agent_chat::{self, SendTurnCommandInput, TurnOrigin},
    database::DatabaseStore,
};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};
use tauri::{AppHandle, Emitter, Manager, Runtime};

#[derive(Clone)]
pub struct SendContext {
    pub origin: TurnOrigin,
    pub provenance: MessageProvenance,
    pub control: Option<NativeControlGuard>,
    pub delivery_id: Option<String>,
    pub generation: Option<u64>,
    /// An already-admitted worker owns this claim, rather than claiming again.
    pub claimed: bool,
    pub release_queue: bool,
    pub(crate) pr_watch: Option<PrWatchDelivery>,
}
impl SendContext {
    pub fn new(origin: TurnOrigin) -> Self {
        let human = origin == TurnOrigin::User;
        Self {
            origin,
            provenance: MessageProvenance {
                origin: Some(origin),
                created_by: if human {
                    CreatedBy::User
                } else {
                    CreatedBy::System
                },
                creation_source: if human {
                    CreationSource::Desktop
                } else {
                    CreationSource::Codemux
                },
                ..Default::default()
            },
            control: None,
            delivery_id: None,
            generation: None,
            claimed: false,
            release_queue: human,
            pr_watch: None,
        }
    }
    pub fn controller(caller: &ControlCaller) -> Self {
        let mut context = Self::new(TurnOrigin::Controller);
        context.provenance.created_by = caller.created_by;
        context.provenance.creation_source = CreationSource::Mcp;
        context
    }
    pub fn is_human(&self) -> bool {
        self.origin == TurnOrigin::User && self.provenance.created_by == CreatedBy::User
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct PrWatchDelivery {
    pub generation: i64,
    pub event_keys: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct DeliveryPayload {
    pub provider: ProviderKind,
    pub input: SendTurnCommandInput,
    pub origin: TurnOrigin,
    pub provenance: MessageProvenance,
    pub caller: Option<ControlCaller>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pr_watch: Option<PrWatchDelivery>,
}

pub(crate) fn select_delivery(
    requested: crate::agent_provider::types::MessageDelivery,
    active: bool,
    safely_steerable: bool,
) -> crate::agent_provider::types::MessageDelivery {
    use crate::agent_provider::types::MessageDelivery;
    if requested != MessageDelivery::Auto {
        requested
    } else if active && safely_steerable {
        MessageDelivery::Steer
    } else {
        MessageDelivery::Queue
    }
}

/// These adapters acknowledge an in-process prompt worker before the native
/// prompt response. The wire boundary records possible submission; only a
/// native prompt response supplies acceptance evidence.
pub(crate) fn deferred_acceptance(provider: ProviderKind) -> bool {
    matches!(
        provider,
        ProviderKind::Cursor | ProviderKind::Grok | ProviderKind::Hermes | ProviderKind::Acp
    )
}

pub(crate) fn notify() {
    wake().notify_one();
}
pub(crate) fn changed<R: Runtime>(app: &AppHandle<R>, thread: &str) {
    let _ = app.emit("agent_chat_delivery_changed", thread);
}
fn wake() -> &'static tokio::sync::Notify {
    static WAKE: OnceLock<tokio::sync::Notify> = OnceLock::new();
    WAKE.get_or_init(Default::default)
}

/// Admission wakes this worker immediately; transitions/startup and a bounded
/// repair sweep cover lost notifications. Claiming is a SQLite compare-and-set.
pub(crate) fn spawn<R: Runtime>(app: AppHandle<R>) {
    let epoch = app.state::<NativeControlState>().epoch.clone();
    if let Err(error) = app.state::<DatabaseStore>().recover_message_deliveries(&epoch) {
        log::error!("Delivery recovery failed: {error}");
    }
    tauri::async_runtime::spawn(async move {
        loop {
            match app.state::<DatabaseStore>().reconcile_pr_watch_deliveries() {
                Ok(threads) => for thread in threads { changed(&app, &thread); },
                Err(error) => log::warn!("PR-watch delivery reconciliation failed: {error}"),
            }
            let rows = app.state::<DatabaseStore>().pending_message_deliveries();
            match rows {
                Ok(rows) => {
                    for row in rows {
                        let owned = app.clone();
                        tauri::async_runtime::spawn(async move {
                            dispatch(&owned, row).await;
                        });
                    }
                }
                Err(error) => log::warn!("Delivery repair failed: {error}"),
            }
            tokio::select! { _ = wake().notified() => {}, _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {} }
        }
    });
}

pub(crate) async fn dispatch<R: Runtime>(
    app: &AppHandle<R>,
    row: crate::database::message_delivery::DeliveryRecord,
) {
    dispatch_admitted(app, row, None).await;
}

pub(crate) async fn dispatch_admitted<R: Runtime>(
    app: &AppHandle<R>,
    row: crate::database::message_delivery::DeliveryRecord,
    mut admitted_control: Option<NativeControlGuard>,
) {
    let db = app.state::<DatabaseStore>();
    let admitted_generation = row.generation;
    // The initial controller dispatch already owns its admission. Publishing
    // it as pending would let the repair worker discard its native fence.
    if admitted_control.is_none() && !db.claim_message_delivery(&row.id).unwrap_or(false) {
        return;
    }
    let _changed = DeliveryChange { app, thread: &row.thread_id };
    let row = match db.message_delivery(&row.id) {
        Ok(Some(row)) => row,
        _ => return,
    };
    if admitted_control.is_some() {
        if row.state != "preparing" {
            let _ = db.fail_message_delivery(&row.id, "Delivery held before initial dispatch");
            return;
        }
        // Only a durable human release advances the admitted Stop fence.
        // Session replacement alone cannot authorize an old operation.
        if let Some((_, generation)) = admitted_control.as_mut().and_then(|control| control.admission.as_mut()) {
            *generation = generation.saturating_add(row.generation.saturating_sub(admitted_generation));
        }
    }
    let payload = match serde_json::from_value::<DeliveryPayload>(
        row.dispatch_payload.unwrap_or(row.payload),
    ) {
        Ok(payload) => payload,
        Err(error) => {
            let _ = db.finish_message_delivery(
                &row.id,
                "held",
                None,
                Some(&format!("Invalid admitted payload: {error}")),
            );
            return;
        }
    };
    let control = admitted_control.or_else(|| payload.caller.map(|caller| {
        NativeControlGuard {
            caller,
            workspace_id: db
                .get_agent_chat_session(&row.thread_id)
                .map(|r| r.workspace_id)
                .unwrap_or_default(),
            thread_id: row.thread_id.clone(),
            provider: payload.provider,
            admission: None,
        }
        .retain_admission(app)
    }));
    if let Some(control) = &control {
        if let Err(error) = control.verify(app) {
            // A definite refusal or cancellation settles this unqueued operation.
            // Temporary binding failures still use the bounded retry path.
            let denied = error.starts_with("permission_ceiling:")
                || error.starts_with("permission_state_unconfirmed:")
                || error.starts_with("grant_revoked:")
                || error.starts_with("cancelled:");
            let state = if denied {
                db.finish_message_delivery(&row.id, "rejected", None, Some(&error)).map(|()| "rejected".to_string())
            } else {
                db.fail_message_delivery(&row.id, &error)
            };
            if state.ok().as_deref() == Some("rejected") {
                warn_held(app, &row.thread_id, &error);
            }
            return;
        }
    }
    let _activity = if payload.origin != TurnOrigin::Controller {
        Some(
            agent_chat::delegation_activity_lock(&row.thread_id)
                .lock_owned()
                .await,
        )
    } else {
        None
    };
    let context = SendContext {
        origin: payload.origin,
        provenance: payload.provenance,
        control,
        delivery_id: Some(row.id.clone()),
        generation: Some(row.generation),
        claimed: true,
        release_queue: false,
        pr_watch: payload.pr_watch,
    };
    if let Err(error) =
        agent_chat::send_turn_with_context(app.clone(), payload.provider, payload.input, context)
            .await
    {
        if db.message_delivery(&row.id).ok().flatten().is_some_and(|row|row.state == "cancelled") {
            log::debug!("Disposed message delivery {}: {error}", row.id);
        } else {
            log::warn!("Message delivery {}: {error}", row.id);
        }
    }
}

struct DeliveryChange<'a, R: Runtime> { app: &'a AppHandle<R>, thread: &'a str }
impl<R: Runtime> Drop for DeliveryChange<'_, R> {
    fn drop(&mut self) { changed(self.app, self.thread); }
}

pub(crate) struct DeliveryCheckpoint<R: Runtime> {
    pub app: AppHandle<R>,
    pub id: String,
    pub thread: String,
    pub provider: ProviderKind,
    pub inner: Option<Arc<dyn TurnDispatchCheckpoint>>,
    pub release_queue: bool,
    pub admitted_generation: u64,
    pub resumed_ids: Vec<String>,
}
impl<R: Runtime> std::fmt::Debug for DeliveryCheckpoint<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DeliveryCheckpoint")
    }
}
#[async_trait::async_trait]
impl<R: Runtime> TurnDispatchCheckpoint for DeliveryCheckpoint<R> {
    fn releases_queue_hold(&self) -> bool { self.release_queue }
    fn queued_delivery_state(&self) -> crate::agent_provider::types::QueuedDeliveryState {
        use crate::agent_provider::types::QueuedDeliveryState::*;
        match self.app.state::<DatabaseStore>().message_delivery(&self.id) {
            Ok(Some(row)) if row.state == "cancelled" => Disposed,
            Ok(Some(row)) if row.state == "held" && row.hold_reason.as_deref() == Some("stop") => Held,
            Ok(Some(row)) if matches!(row.state.as_str(), "held" | "held_unknown" | "rejected") => Blocked,
            Ok(Some(_)) => Ready,
            _ => Held,
        }
    }
    fn authorize_queue_promotion(&self) -> Result<(), crate::agent_provider::ProviderError> {
        if self.queued_delivery_state() != crate::agent_provider::types::QueuedDeliveryState::Ready {
            return Err(crate::agent_provider::ProviderError::ValidationError {message:"Queued message is still held or disposed; inspect its delivery error before sending.".into()});
        }
        self.verify_authority()
    }
    fn authorize_dispatch(&self) -> Result<(), crate::agent_provider::ProviderError> {
        self.verify_authority()?;
        self.app.state::<DatabaseStore>().message_delivery_submitting(&self.id)
            .map_err(|message| crate::agent_provider::ProviderError::ValidationError { message })
    }
    fn verify_dispatch(&self) -> Result<(), crate::agent_provider::ProviderError> {
        self.verify_authority()
    }
    async fn prepare(&self) {
        if let Some(inner) = &self.inner { inner.prepare().await; }
    }
    async fn commit(&self) {
        if let Some(inner) = &self.inner { inner.commit().await; }
    }
    async fn abort(&self) {
        let db = self.app.state::<DatabaseStore>();
        let _ = db.rehold_message_deliveries(&self.resumed_ids);
        let reason = "Provider submission failed; inspect acceptance evidence before resending";
        let already_held = db.message_delivery(&self.id).ok().flatten().is_some_and(|row| row.state == "held_unknown");
        if db.fail_message_delivery(&self.id, reason).ok().as_deref() == Some("held_unknown") && !already_held {
            warn_held(&self.app, &self.thread, reason);
        }
        if let Some(inner) = &self.inner { inner.abort().await; }
    }
    async fn defer(&self) {
        if let Some(inner) = &self.inner { inner.defer().await; }
    }
    async fn accepted(&self, turn: &crate::agent_provider::TurnId) {
        if let Err(error) = self.app.state::<DatabaseStore>().accept_message_delivery_prompt(&self.id, &turn.0) {
            log::error!("Could not record prompt acceptance: {error}");
        }
        if let Some(inner) = &self.inner { inner.accepted(turn).await; }
        changed(&self.app, &self.thread);
    }
}
impl<R: Runtime> DeliveryCheckpoint<R> {
    fn verify_authority(&self) -> Result<(), crate::agent_provider::ProviderError> {
        let db = self.app.state::<DatabaseStore>();
        let record = db.message_delivery(&self.id).map_err(|message|crate::agent_provider::ProviderError::ValidationError {message})?;
        let released_stops = record.as_ref().and_then(|row|row.generation.checked_sub(self.admitted_generation)).unwrap_or(0);
        if let Some(inner) = &self.inner {
            if let Err(error) = inner.authorize_rearmed_dispatch(released_stops) {
                let _ = db.refuse_message_delivery(&self.id, &error.to_string());
                changed(&self.app, &self.thread);
                return Err(error);
            }
        }
        let binding = db.get_agent_chat_session(&self.thread);
        let valid = binding.as_ref().is_some_and(|record| {
            record.imported_from.is_none()
                && serde_json::from_value::<ProviderKind>(serde_json::json!(record.provider)).ok()
                    == Some(self.provider)
        }) && self
            .app
            .try_state::<crate::state::AppStateStore>()
            .is_none_or(|state| {
                binding.as_ref().is_some_and(|record| {
                    state.find_workspace(&record.workspace_id).is_some()
                        && state.agent_chat_pane_id_for_thread(&self.thread).is_some()
                })
            });
        if !valid {
            let message =
                "Delivery held: target was deleted, archived, unbound or changed provider";
            let _ = db.fail_message_delivery(&self.id, message);
            return Err(crate::agent_provider::ProviderError::ValidationError {
                message: message.into(),
            });
        }
        Ok(())
    }
}

pub(crate) fn warn_held<R: Runtime>(app: &AppHandle<R>, thread: &str, error: &str) {
    changed(app, thread);
    if app.state::<DatabaseStore>().thread_message_delivery_attention(thread).is_ok_and(|rows| rows.is_empty()) { return; }
    agent_chat::forward_event(
        app,
        crate::agent_provider::ProviderRuntimeEvent::RuntimeWarning {
            thread_id: Some(crate::agent_provider::ThreadId(thread.into())),
            message: format!(
                "Message delivery needs attention: {error}. Automatic resend is held."
            ),
            original_payload: None,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_provider::types::MessageDelivery::*;
    #[test]
    fn orchestration_message_auto_selection_never_interrupts() {
        assert_eq!(select_delivery(Auto, false, false), Queue);
        assert_eq!(select_delivery(Auto, true, true), Steer);
        assert_eq!(select_delivery(Auto, true, false), Queue);
        assert_eq!(select_delivery(Queue, true, true), Queue);
        assert_eq!(select_delivery(Steer, true, false), Steer);
        let context = SendContext::controller(&ControlCaller::trusted());
        assert_eq!(context.provenance.created_by, CreatedBy::User);
        assert_eq!(context.provenance.creation_source, CreationSource::Mcp);
        assert!(!context.is_human());
    }
}
