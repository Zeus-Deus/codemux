//! Commands for conversation-owned questions. Approval callbacks are unchanged.
use std::sync::Arc;

use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use super::agent_chat::{
    ensure_live_session_mode, fan_out_to_thread_channels, feature_flag_on, lookup_provider,
    run_checkpoints_enabled, AgentChatEventPayload, GitTurnDispatchCheckpoint, ProviderRegistry,
};
use crate::agent_provider::{
    AnswerQuestionInput, ProviderKind, ProviderRuntimeEvent, QuestionDelivery, QuestionDeliveryError,
    QuestionResolution, ThreadId, TurnDispatchCheckpoint,
};
use crate::database::DatabaseStore;
use crate::observability::ObservabilityStore;

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum QuestionAction {
    Answer {
        answers: Vec<String>,
        submission_id: String,
        #[serde(default)]
        retry_unknown: bool,
        #[serde(default)]
        permission_mode_override: Option<String>,
    },
    Dismiss,
    Reopen,
    Reconcile,
}

pub(super) fn publish_attention<R: Runtime>(app: &AppHandle<R>) {
    let db: State<'_, DatabaseStore> = app.state();
    if let Ok(snapshot) = db.async_question_attention() {
        let _ = app.emit("agent-chat-question-attention", snapshot);
    }
}

#[tauri::command]
pub fn agent_chat_question_attention(
    db: State<'_, DatabaseStore>,
) -> Result<crate::database::async_questions::QuestionAttention, String> {
    db.async_question_attention()
}

fn publish<R: Runtime>(
    app: &AppHandle<R>,
    thread_id: &ThreadId,
    question_id: &str,
    resolution: &QuestionResolution,
    row_id: i64,
) {
    let event = ProviderRuntimeEvent::QuestionResolved {
        thread_id: thread_id.clone(),
        question_id: question_id.into(),
        resolution: resolution.clone(),
    };
    publish_attention(app);
    // Resolutions reach clients here, not through `forward_event`, so tell
    // delegation directly: a delegated child waiting on this question is
    // working again (or done) once it is answered or dismissed.
    super::delegation::observe(app, &event, Some(row_id));
    fan_out_to_thread_channels(
        app,
        thread_id,
        &AgentChatEventPayload {
            thread_id: thread_id.clone(),
            event,
            persisted_id: Some(row_id),
            retained_images: Vec::new(),
        },
    );
}

fn change<R: Runtime>(
    app: &AppHandle<R>,
    thread_id: &ThreadId,
    question_id: &str,
    previous: &QuestionResolution,
    next: &QuestionResolution,
) -> Result<Option<i64>, String> {
    let db: State<'_, DatabaseStore> = app.state();
    let row_id = db.change_async_question(&thread_id.0, question_id, previous, next)?;
    if let Some(id) = row_id {
        publish(app, thread_id, question_id, next, id);
    }
    Ok(row_id)
}

/// A question answer can start a turn without a message-delivery row. Its
/// authority must be captured before resume and retained through preparation.
struct AnswerCheckpoint<R: Runtime> {
    app: AppHandle<R>,
    thread: String,
    generation: u64,
    inner: Option<Arc<dyn TurnDispatchCheckpoint>>,
}
impl<R: Runtime> std::fmt::Debug for AnswerCheckpoint<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("AnswerCheckpoint") }
}
#[async_trait::async_trait]
impl<R: Runtime> TurnDispatchCheckpoint for AnswerCheckpoint<R> {
    fn native_stop_generation(&self) -> Result<u64, crate::agent_provider::ProviderError> {
        self.verify_dispatch()?;
        Ok(self.generation)
    }
    fn verify_dispatch(&self) -> Result<(), crate::agent_provider::ProviderError> {
        self.app.state::<DatabaseStore>().verify_delivery_generation(&self.thread, self.generation)
            .map_err(|message| crate::agent_provider::ProviderError::ValidationError { message })?;
        if let Some(inner) = &self.inner { inner.verify_dispatch()?; }
        Ok(())
    }
    fn authorize_dispatch(&self) -> Result<(), crate::agent_provider::ProviderError> {
        self.verify_dispatch()?;
        if let Some(inner) = &self.inner { inner.authorize_dispatch()?; }
        Ok(())
    }
    async fn prepare(&self) { if let Some(inner) = &self.inner { inner.prepare().await; } }
    async fn commit(&self) { if let Some(inner) = &self.inner { inner.commit().await; } }
    async fn abort(&self) { if let Some(inner) = &self.inner { inner.abort().await; } }
    async fn accepted(&self, turn: &crate::agent_provider::TurnId) { if let Some(inner) = &self.inner { inner.accepted(turn).await; } }
}

#[tauri::command]
pub async fn agent_chat_answer_question<R: Runtime>(
    app: AppHandle<R>,
    thread_id: ThreadId,
    question_id: String,
    action: QuestionAction,
) -> Result<QuestionResolution, String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;
    let db: State<'_, DatabaseStore> = app.state();
    let record = db
        .get_agent_chat_session(&thread_id.0)
        .ok_or("Conversation no longer exists.")?;
    let provider_kind: ProviderKind = serde_json::from_value(serde_json::json!(record.provider))
        .map_err(|_| "Unknown conversation provider.")?;
    let generation = db.delivery_generation(&thread_id.0)?.0;
    let registry: State<'_, ProviderRegistry> = app.state();
    let provider = lookup_provider(&registry, provider_kind).await?;
    if !provider.supports_async_questions() {
        return Err("This provider does not support asynchronous questions.".into());
    }
    let (question, previous) = db.async_question(&thread_id.0, &question_id)?;

    let permission_mode_override = match &action {
        QuestionAction::Answer { permission_mode_override, .. } => permission_mode_override.clone(),
        _ => None,
    };
    let (submission_id, answers, reconcile) = match action {
        QuestionAction::Dismiss | QuestionAction::Reopen => {
            let next = match (&action, &previous) {
                (
                    QuestionAction::Dismiss,
                    QuestionResolution::Pending | QuestionResolution::Failed { .. },
                ) => QuestionResolution::Dismissed,
                (QuestionAction::Reopen, QuestionResolution::Dismissed) => {
                    QuestionResolution::Pending
                }
                _ => return Ok(previous),
            };
            change(&app, &thread_id, &question_id, &previous, &next)?;
            return Ok(db.async_question(&thread_id.0, &question_id)?.1);
        }
        QuestionAction::Reconcile => match &previous {
            QuestionResolution::Submitting {
                submission_id,
                answers,
            }
            | QuestionResolution::Unknown {
                submission_id,
                answers,
                ..
            } => (submission_id.clone(), answers.clone(), true),
            _ => return Ok(previous),
        },
        QuestionAction::Answer {
            answers,
            submission_id,
            retry_unknown,
            ..
        } => {
            // Reject arbitrary oversized identifiers before they reach RPC or DB.
            uuid::Uuid::parse_str(&submission_id).map_err(|_| "Invalid submission ID.")?;
            question.answer_text(&answers)?;
            if !matches!(
                previous,
                QuestionResolution::Pending | QuestionResolution::Failed { .. }
            ) && !(retry_unknown && matches!(previous, QuestionResolution::Unknown { .. }))
            {
                return Ok(previous);
            }
            (submission_id, answers, false)
        }
    };
    // Claim before any await, including resume. A second window sees submitting.
    let submitting = QuestionResolution::Submitting {
        submission_id: submission_id.clone(),
        answers: answers.clone(),
    };
    if !reconcile && change(&app, &thread_id, &question_id, &previous, &submitting)?.is_none() {
        return Ok(db.async_question(&thread_id.0, &question_id)?.1);
    }
    let expected = if reconcile { previous } else { submitting };
    let resume = ensure_live_session_mode(&app, provider_kind, &thread_id, true).await;
    let result = match resume {
        Err(message) => if reconcile { Err(QuestionDeliveryError::Unknown(message)) } else { Err(QuestionDeliveryError::Rejected(message)) },
        Ok(()) if reconcile => match provider.find_question_answer(thread_id.clone(), question.target.clone(), submission_id.clone()).await {
            Ok(Some(delivery)) => Ok(delivery),
            Ok(None) => Err(QuestionDeliveryError::Unknown("The answer was not found in the available provider history. Sending it again may duplicate it.".into())),
            Err(message) => Err(QuestionDeliveryError::Unknown(message)),
        },
        Ok(()) => {
            let checkpoint: Option<Arc<dyn TurnDispatchCheckpoint>> = if run_checkpoints_enabled() && provider.capabilities().supports_conversation_rollback {
                record.cwd.clone().map(|cwd| Arc::new(GitTurnDispatchCheckpoint::new(app.clone(), thread_id.0.clone(), record.workspace_id, cwd, Some(submission_id.clone()))) as Arc<dyn TurnDispatchCheckpoint>)
            } else { None };
            let checkpoint = Some(Arc::new(AnswerCheckpoint {app:app.clone(),thread:thread_id.0.clone(),generation,inner:checkpoint}) as Arc<dyn TurnDispatchCheckpoint>);
            let checkpoint = super::turn_diff::wrap_dispatch_checkpoint(&app, &thread_id.0, record.cwd, Some(&submission_id), checkpoint);
            let delivered = provider.answer_question(AnswerQuestionInput { thread_id: thread_id.clone(), question, answers: answers.clone(), submission_id: submission_id.clone(), checkpoint, permission_mode_override }).await;
            if let Ok(QuestionDelivery::NewTurn { turn_id }) = &delivered {
                super::turn_diff::on_turn_bound(&app, &thread_id.0, Some(&submission_id), &turn_id.0);
            }
            delivered
        }
    };
    let resolution = match result {
        Ok(delivery) => QuestionResolution::Answered {
            submission_id: submission_id.clone(),
            answers,
            delivery,
        },
        Err(QuestionDeliveryError::Rejected(message)) => QuestionResolution::Failed {
            submission_id: submission_id.clone(),
            answers,
            message,
        },
        Err(QuestionDeliveryError::Unknown(message)) => QuestionResolution::Unknown {
            submission_id: submission_id.clone(),
            answers,
            message,
        },
    };
    let row_id = change(&app, &thread_id, &question_id, &expected, &resolution)?;
    if row_id.is_none() && matches!(resolution, QuestionResolution::Answered { .. }) {
        let current = db.async_question(&thread_id.0, &question_id)?.1;
        if matches!(&current, QuestionResolution::Unknown { submission_id: id, .. } | QuestionResolution::Submitting { submission_id: id, .. } if id == &submission_id)
        {
            change(&app, &thread_id, &question_id, &current, &resolution)?;
        }
    }
    // New turns retain the checkpoint captured immediately before dispatch.
    // Binding to this late acknowledgment would retain output from the turn
    // being reverted. Its client nonce already associates the reply bubble.
    Ok(db.async_question(&thread_id.0, &question_id)?.1)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::agent_provider::{AgentProvider, UserQuestion, UserQuestionSet};

    #[test]
    fn review600_async_questions_dispatch_wrappers_reject_stop_after_admission() {
        let app = tauri::test::mock_app();
        app.manage(DatabaseStore::new_in_memory());
        let db = app.state::<DatabaseStore>();
        db.stop_message_deliveries("answer").unwrap();
        db.resume_message_delivery_thread("answer").unwrap();
        let generation = db.delivery_generation("answer").unwrap().0;
        let answer = Arc::new(AnswerCheckpoint {app:app.handle().clone(),thread:"answer".into(),generation,inner:None});
        let wrapped = super::super::turn_diff::wrap_dispatch_checkpoint(app.handle(), "answer", Some("/not-a-repo".into()), Some("answer-nonce"), Some(answer)).unwrap();
        assert_eq!(wrapped.native_stop_generation().unwrap(), generation);
        db.stop_message_deliveries("answer").unwrap();
        db.resume_message_delivery_thread("answer").unwrap();
        assert!(wrapped.authorize_dispatch().is_err());
        assert!(wrapped.native_stop_generation().is_err());
    }

    #[tokio::test]
    async fn review600_async_questions_idle_answer_after_stop_resume_keeps_durable_generation() {
        let binary = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join("fake_codex_app_server");
        assert!(binary.exists(), "build the fake_codex_app_server test fixture first");
        for capture_diff in [false, true] {
            let app = tauri::test::mock_app();
            let root = tempfile::tempdir().unwrap();
            let thread = format!("review600-answer-{capture_diff}");
            let db = DatabaseStore::new_in_memory();
            let cwd = capture_diff.then(|| root.path().to_string_lossy().to_string());
            db.upsert_agent_chat_session(&thread, "workspace", cwd.as_deref(), "codex").unwrap();
            let question = UserQuestionSet { id:"q".into(),target:"c-1".into(),source_item_id:"q-1".into(),source_turn_id:"old-turn".into(),text:String::new(),subagent_id:None,questions:vec![UserQuestion {title:"Which storage?".into(),options:vec![]}] };
            db.record_async_question(&thread, &question).unwrap();
            db.stop_message_deliveries(&thread).unwrap();
            db.resume_message_delivery_thread(&thread).unwrap();
            let expected_generation = db.delivery_generation(&thread).unwrap().0;
            assert!(expected_generation > 0);
            let provider = Arc::new(crate::agent_provider::codex::CodexAgentProvider::new(crate::agent_provider::codex::CodexProviderConfig {codex_binary:binary.clone(), ..Default::default()}));
            let input = serde_json::from_value(serde_json::json!({"thread_id":thread,"cwd":root.path(),"permission_mode":"danger-full-access","additional_directories":[],"env":{"FAKE_CODEX_ASYNC_MODE":"idle"}})).unwrap();
            provider.start_session(input).await.unwrap();
            let registry = ProviderRegistry::new();
            registry.set_codex(provider.clone()).await;
            app.manage(registry);
            app.manage(db);
            let observability = ObservabilityStore::default();
            let mut flags = observability.feature_flags(); flags.enable_agent_chat = true; observability.set_feature_flags(flags);
            app.manage(observability);
            app.manage(super::super::agent_chat::AgentChatChannelRegistry::default());
            let result = agent_chat_answer_question(app.handle().clone(), ThreadId(thread.clone()), "q".into(), QuestionAction::Answer {answers:vec!["SQLite".into()],submission_id:uuid::Uuid::new_v4().to_string(),retry_unknown:false,permission_mode_override:None}).await.unwrap();
            let identity = crate::delegation::authority::NativeAuthority::current_permit(&thread).identity();
            provider.stop_session(ThreadId(thread.clone())).await.unwrap();
            assert!(matches!(result, QuestionResolution::Answered {delivery:QuestionDelivery::NewTurn {..},..}), "{result:?}");
            let identity = identity.unwrap();
            assert_eq!(identity.stop_generation, expected_generation, "the answer must retain admission's durable Stop generation");
            app.state::<DatabaseStore>().verify_delivery_generation(&thread, identity.stop_generation).unwrap();
        }
    }
}
