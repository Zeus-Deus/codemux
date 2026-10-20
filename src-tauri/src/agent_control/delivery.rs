//! Persisted message delivery. Native session/queue ownership stays in agent_chat.
use super::{ControlCaller, NativeControlGuard, NativeControlState};
use crate::{
    agent_provider::{message::*, ProviderKind, TurnDispatchCheckpoint},
    commands::agent_chat::{self, SendTurnCommandInput, TurnOrigin},
    database::DatabaseStore,
};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};
use tauri::{AppHandle, Manager, Runtime};

#[derive(Clone)]
pub struct SendContext {
    pub origin: TurnOrigin,
    pub provenance: MessageProvenance,
    pub control: Option<NativeControlGuard>,
    pub delivery_id: Option<String>,
    pub generation: Option<u64>,
    /// An already-admitted worker owns this claim, rather than claiming again.
    pub claimed: bool,
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
pub(crate) struct DeliveryPayload {
    pub provider: ProviderKind,
    pub input: SendTurnCommandInput,
    pub origin: TurnOrigin,
    pub provenance: MessageProvenance,
    pub caller: Option<ControlCaller>,
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
/// prompt response. Treat that acknowledgement as possible submission until
/// exact-turn completion supplies acceptance evidence.
pub(crate) fn deferred_acceptance(provider: ProviderKind) -> bool {
    matches!(
        provider,
        ProviderKind::Cursor | ProviderKind::Grok | ProviderKind::Hermes | ProviderKind::Acp
    )
}

pub(crate) fn notify() {
    wake().notify_one();
}
fn wake() -> &'static tokio::sync::Notify {
    static WAKE: OnceLock<tokio::sync::Notify> = OnceLock::new();
    WAKE.get_or_init(Default::default)
}

/// Admission wakes this worker immediately; transitions/startup and a bounded
/// repair sweep cover lost notifications. Claiming is a SQLite compare-and-set.
pub(crate) fn spawn<R: Runtime>(app: AppHandle<R>) {
    let epoch = app.state::<NativeControlState>().epoch.clone();
    match app
        .state::<DatabaseStore>()
        .recover_message_deliveries(&epoch)
    {
        Ok(()) => {
            if let Ok(rows) = app
                .state::<DatabaseStore>()
                .message_delivery_recovery_attention()
            {
                for (thread, error) in rows {
                    warn_held(&app, &thread, &error);
                }
            }
        }
        Err(error) => log::error!("Delivery recovery failed: {error}"),
    }
    tauri::async_runtime::spawn(async move {
        loop {
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
    let db = app.state::<DatabaseStore>();
    if !db.claim_message_delivery(&row.id).unwrap_or(false) {
        return;
    }
    let row = match db.message_delivery(&row.id) {
        Ok(Some(row)) => row,
        _ => return,
    };
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
    let control = payload.caller.map(|caller| {
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
    });
    if let Some(control) = &control {
        if let Err(error) = control.verify(app) {
            let _ = db.finish_message_delivery(&row.id, "held", None, Some(&error));
            warn_held(app, &row.thread_id, &error);
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
    };
    if let Err(error) =
        agent_chat::send_turn_with_context(app.clone(), payload.provider, payload.input, context)
            .await
    {
        log::warn!("Message delivery {}: {error}", row.id);
    }
}

pub(crate) struct DeliveryCheckpoint<R: Runtime> {
    pub app: AppHandle<R>,
    pub id: String,
    pub thread: String,
    pub provider: ProviderKind,
    pub generation: u64,
    pub inner: Option<Arc<dyn TurnDispatchCheckpoint>>,
}
impl<R: Runtime> std::fmt::Debug for DeliveryCheckpoint<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DeliveryCheckpoint")
    }
}
#[async_trait::async_trait]
impl<R: Runtime> TurnDispatchCheckpoint for DeliveryCheckpoint<R> {
    fn authorize_dispatch(&self) -> Result<(), crate::agent_provider::ProviderError> {
        if let Some(inner) = &self.inner {
            inner.authorize_dispatch()?;
        }
        let db = self.app.state::<DatabaseStore>();
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
            let _ = db.finish_message_delivery(&self.id, "held", None, Some(message));
            return Err(crate::agent_provider::ProviderError::ValidationError {
                message: message.into(),
            });
        }
        db.verify_delivery_generation(&self.thread, self.generation)
            .and_then(|_| db.message_delivery_submitting(&self.id))
            .map_err(|message| crate::agent_provider::ProviderError::ValidationError { message })
    }
    async fn prepare(&self) {
        if let Some(inner) = &self.inner {
            inner.prepare().await;
        }
    }
    async fn commit(&self) {
        if let Some(inner) = &self.inner {
            inner.commit().await;
        }
    }
    async fn abort(&self) {
        let db = self.app.state::<DatabaseStore>();
        let reason = "Provider submission failed; inspect acceptance evidence before resending";
        let already_held = db
            .message_delivery(&self.id)
            .ok()
            .flatten()
            .is_some_and(|row| row.state == "held_unknown");
        if db.fail_message_delivery(&self.id, reason).ok().as_deref() == Some("held_unknown")
            && !already_held
        {
            warn_held(&self.app, &self.thread, reason);
        }
        if let Some(inner) = &self.inner {
            inner.abort().await;
        }
    }
}

pub(crate) fn warn_held<R: Runtime>(app: &AppHandle<R>, thread: &str, error: &str) {
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
