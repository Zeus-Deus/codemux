//! Conversation-owned clarification questions. They are not approval callbacks.
use super::{ThreadId, TurnDispatchCheckpoint, TurnId};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserQuestion {
    pub title: String,
    #[serde(default)]
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserQuestionSet {
    pub id: String,
    /// Opaque provider-owned conversation route. Never taken from answer IPC.
    pub target: String,
    pub source_item_id: String,
    pub source_turn_id: String,
    pub text: String,
    pub questions: Vec<UserQuestion>,
    #[serde(default)]
    pub subagent_id: Option<String>,
}

impl UserQuestionSet {
    pub fn answer_text(&self, answers: &[String]) -> Result<String, String> {
        if answers.len() != self.questions.len()
            || answers
                .iter()
                .any(|a| a.trim().is_empty() || a.len() > 32_000)
        {
            return Err("Answer each question with at most 32,000 bytes of text.".into());
        }
        Ok(self
            .questions
            .iter()
            .zip(answers)
            .map(|(q, a)| format!("Question: {}\nAnswer: {}", q.title, a.trim()))
            .collect::<Vec<_>>()
            .join("\n\n"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuestionDelivery {
    Inflight { turn_id: TurnId },
    NewTurn { turn_id: TurnId },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum QuestionResolution {
    Pending,
    Submitting {
        submission_id: String,
        answers: Vec<String>,
    },
    Answered {
        submission_id: String,
        answers: Vec<String>,
        delivery: QuestionDelivery,
    },
    Dismissed,
    Failed {
        submission_id: String,
        answers: Vec<String>,
        message: String,
    },
    Unknown {
        submission_id: String,
        answers: Vec<String>,
        message: String,
    },
}

#[derive(Debug)]
pub struct AnswerQuestionInput {
    pub thread_id: ThreadId,
    pub question: UserQuestionSet,
    pub answers: Vec<String>,
    pub submission_id: String,
    pub checkpoint: Option<Arc<dyn TurnDispatchCheckpoint>>,
}

/// A rejection is known not to have been accepted. Transport failures after
/// dispatch must remain uncertain, so callers cannot blindly retry them.
#[derive(Debug)]
pub enum QuestionDeliveryError {
    Rejected(String),
    Unknown(String),
}
