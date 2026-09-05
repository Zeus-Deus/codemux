//! Commands for conversation-owned questions. Approval callbacks are unchanged.
use super::*;
use crate::agent_provider::{AnswerQuestionInput, QuestionDeliveryError, QuestionResolution};

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum QuestionAction {
    Answer {
        answers: Vec<String>,
        submission_id: String,
        #[serde(default)]
        retry_unknown: bool,
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
    fan_out_to_thread_channels(
        app,
        thread_id,
        &AgentChatEventPayload {
            thread_id: thread_id.clone(),
            event,
            persisted_id: Some(row_id),
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
    let registry: State<'_, ProviderRegistry> = app.state();
    let provider = lookup_provider(&registry, provider_kind).await?;
    if !provider.supports_async_questions() {
        return Err("This provider does not support asynchronous questions.".into());
    }
    let (question, previous) = db.async_question(&thread_id.0, &question_id)?;

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
                record.cwd.map(|cwd| Arc::new(GitTurnDispatchCheckpoint::new(app.clone(), thread_id.0.clone(), record.workspace_id, cwd, Some(submission_id.clone()))) as Arc<dyn TurnDispatchCheckpoint>)
            } else { None };
            provider.answer_question(AnswerQuestionInput { thread_id: thread_id.clone(), question, answers: answers.clone(), submission_id: submission_id.clone(), checkpoint }).await
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
