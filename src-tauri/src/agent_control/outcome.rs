//! How a native run ended, shared by thread waits and delegation. A delegated
//! child and the same chat seen through `thread_wait` never disagree about
//! done, failed or needs input. This only classifies; it tracks no events.
use crate::agent_provider::TurnStatus;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RunState {
    Running,
    NeedsInput,
    Done,
    Failed,
}

#[cfg(test)]
impl RunState {
    /// Every state. A new state belongs here too: the guide's wait-states
    /// table is checked against this list (`docs_tests`).
    pub(crate) const ALL: [Self; 4] = [Self::Running, Self::NeedsInput, Self::Done, Self::Failed];
}

/// A run's terminal status as Codemux recorded it, live or persisted.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TurnEnd<'a> {
    pub kind: Option<&'a str>,
    pub subtype: Option<&'a str>,
    pub message: Option<&'a str>,
}

impl<'a> TurnEnd<'a> {
    pub(crate) fn of(status: &'a TurnStatus) -> Self {
        let (kind, subtype, message) = match status {
            TurnStatus::Success => ("success", None, None),
            TurnStatus::Error { subtype, message } => ("error", Some(subtype.as_str()), Some(message.as_str())),
            TurnStatus::MaxTurns => ("max_turns", None, None),
            TurnStatus::MaxBudget => ("max_budget", None, None),
        };
        Self { kind: Some(kind), subtype, message }
    }

    /// A persisted `turn_completed` status, or the safe `last_turn.status`.
    pub(crate) fn of_json(status: &'a Value) -> Self {
        Self { kind: status["kind"].as_str(), subtype: status["subtype"].as_str(), message: status["message"].as_str() }
    }

    fn outcome(&self) -> (&'static str, &'static str) {
        super::state::turn_outcome(self.kind, self.subtype)
    }

    pub(crate) fn succeeded(&self) -> bool {
        self.outcome().0 == "completed"
    }
}

/// Why a finished run did not succeed. `code()` is safe for any caller;
/// `message` may quote a provider diagnostic and never leaves Codemux.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Failure<'a> {
    NoFinalMessage,
    UsageLimit,
    /// `outcome` is `turn_outcome`'s safe code: error, interrupted, max_turns or max_budget.
    Turn { outcome: &'static str, message: Option<&'a str> },
    /// Codemux never saw the run end.
    WentIdle { last_text: Option<&'a str> },
}

impl Failure<'_> {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::NoFinalMessage => "no_final_message",
            Self::UsageLimit => "usage_limit",
            Self::Turn { outcome, .. } => *outcome,
            Self::WentIdle { .. } => "went_idle",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RunEnd<'a> {
    Done(&'a str),
    Failed(Failure<'a>),
}

/// How a run that stopped reads. `None` fails closed: a report Codemux cannot
/// vouch for is not a success.
pub(crate) fn finished<'a>(end: Option<TurnEnd<'a>>, last_text: Option<&'a str>) -> RunEnd<'a> {
    let text = last_text.map(str::trim).filter(|text| !text.is_empty());
    let Some(end) = end else { return RunEnd::Failed(Failure::WentIdle { last_text: text }) };
    if end.succeeded() {
        return text.map_or(RunEnd::Failed(Failure::NoFinalMessage), RunEnd::Done);
    }
    if end.subtype == Some(crate::agent_provider::events::RATE_LIMIT_SUBTYPE) {
        return RunEnd::Failed(Failure::UsageLimit);
    }
    RunEnd::Failed(Failure::Turn { outcome: end.outcome().1, message: end.message })
}

/// Waiting on a person (an open approval, question or secret card), unless
/// the run already failed: nobody will take that answer.
pub(crate) fn awaits_user(run_failed: bool, open_requests: bool) -> bool {
    !run_failed && open_requests
}

pub(crate) struct RunFacts<'a> {
    /// The run stopped: thread_wait's legacy settle rule, or its session went
    /// idle without recording an end.
    pub ended: bool,
    pub end: Option<TurnEnd<'a>>,
    pub last_text: Option<&'a str>,
    pub open_requests: bool,
    /// An armed usage-limit resume continues the work (delegation's child_busy rule).
    pub resume_armed: bool,
}

pub(crate) fn run_state<'a>(facts: &RunFacts<'a>) -> (RunState, Option<Failure<'a>>) {
    let failed = facts.end.is_some_and(|end| !end.succeeded());
    if awaits_user(failed, facts.open_requests) {
        return (RunState::NeedsInput, None);
    }
    if !facts.ended || facts.resume_armed {
        return (RunState::Running, None);
    }
    match finished(facts.end, facts.last_text) {
        RunEnd::Done(_) => (RunState::Done, None),
        RunEnd::Failed(failure) => (RunState::Failed, Some(failure)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_provider::events::RATE_LIMIT_SUBTYPE;

    fn error(subtype: &str, message: &str) -> TurnStatus {
        TurnStatus::Error { subtype: subtype.into(), message: message.into() }
    }

    fn facts<'a>(ended: bool, end: Option<TurnEnd<'a>>, last_text: Option<&'a str>, open_requests: bool, resume_armed: bool) -> RunFacts<'a> {
        RunFacts { ended, end, last_text, open_requests, resume_armed }
    }

    #[test]
    fn finished_follows_the_delegation_rules() {
        let success = TurnStatus::Success;
        assert_eq!(finished(Some(TurnEnd::of(&success)), Some("  Done: PONG  ")), RunEnd::Done("Done: PONG"));
        assert_eq!(finished(Some(TurnEnd::of(&success)), Some("   ")), RunEnd::Failed(Failure::NoFinalMessage));
        assert_eq!(finished(Some(TurnEnd::of(&success)), None), RunEnd::Failed(Failure::NoFinalMessage));
        let limit = error(RATE_LIMIT_SUBTYPE, "rate limited");
        assert_eq!(finished(Some(TurnEnd::of(&limit)), Some("partial")), RunEnd::Failed(Failure::UsageLimit));
        let interrupted = error("interrupted", "Stopped");
        assert_eq!(
            finished(Some(TurnEnd::of(&interrupted)), None),
            RunEnd::Failed(Failure::Turn { outcome: "interrupted", message: Some("Stopped") })
        );
        let provider = error("provider_failure", "boom");
        assert_eq!(
            finished(Some(TurnEnd::of(&provider)), None),
            RunEnd::Failed(Failure::Turn { outcome: "error", message: Some("boom") })
        );
        // An error whose subtype only names a limit is still an error, as before.
        let named = error("max_turns", "too many");
        assert_eq!(
            finished(Some(TurnEnd::of(&named)), None),
            RunEnd::Failed(Failure::Turn { outcome: "error", message: Some("too many") })
        );
        assert_eq!(
            finished(Some(TurnEnd::of(&TurnStatus::MaxTurns)), Some("x")),
            RunEnd::Failed(Failure::Turn { outcome: "max_turns", message: None })
        );
        assert_eq!(
            finished(Some(TurnEnd::of(&TurnStatus::MaxBudget)), Some("x")),
            RunEnd::Failed(Failure::Turn { outcome: "max_budget", message: None })
        );
        assert_eq!(finished(None, Some(" halfway ")), RunEnd::Failed(Failure::WentIdle { last_text: Some("halfway") }));
        assert_eq!(finished(None, None), RunEnd::Failed(Failure::WentIdle { last_text: None }));
        // Persisted and live statuses read the same.
        let persisted = serde_json::to_value(&interrupted).unwrap();
        assert_eq!(finished(Some(TurnEnd::of_json(&persisted)), None), finished(Some(TurnEnd::of(&interrupted)), None));
        let safe = serde_json::json!({"kind": "interrupted"});
        assert_eq!(
            finished(Some(TurnEnd::of_json(&safe)), None),
            RunEnd::Failed(Failure::Turn { outcome: "interrupted", message: None })
        );
    }

    #[test]
    fn run_state_prefers_input_unless_the_run_failed() {
        let success = TurnStatus::Success;
        let failed = error("provider_failure", "boom");
        let limit = error(RATE_LIMIT_SUBTYPE, "rate limited");
        let state = |f: RunFacts<'_>| run_state(&f).0;
        assert_eq!(state(facts(true, Some(TurnEnd::of(&failed)), None, true, false)), RunState::Failed);
        assert_eq!(state(facts(true, Some(TurnEnd::of(&success)), Some("ok"), true, false)), RunState::NeedsInput);
        assert_eq!(state(facts(false, None, None, true, false)), RunState::NeedsInput);
        assert_eq!(state(facts(false, None, None, false, false)), RunState::Running);
        assert_eq!(state(facts(true, Some(TurnEnd::of(&limit)), None, false, true)), RunState::Running);
        assert_eq!(state(facts(true, Some(TurnEnd::of(&limit)), None, false, false)), RunState::Failed);
        assert_eq!(state(facts(true, Some(TurnEnd::of(&success)), Some("ok"), false, false)), RunState::Done);
        assert_eq!(
            run_state(&facts(true, Some(TurnEnd::of(&success)), None, false, false)).1.map(|f| f.code()),
            Some("no_final_message")
        );
        assert_eq!(run_state(&facts(true, None, Some("x"), false, false)).1.map(|f| f.code()), Some("went_idle"));
    }

    #[test]
    fn failure_codes_never_carry_provider_messages() {
        const CODES: [&str; 7] = ["error", "interrupted", "max_turns", "max_budget", "usage_limit", "no_final_message", "went_idle"];
        let secret = "synthetic private diagnostic";
        let statuses = [
            error("provider_failure", secret),
            error("interrupted", secret),
            error(RATE_LIMIT_SUBTYPE, secret),
            error("", secret),
            TurnStatus::MaxTurns,
            TurnStatus::MaxBudget,
            TurnStatus::Success,
        ];
        for status in &statuses {
            for text in [None, Some(secret), Some("")] {
                for end in [Some(TurnEnd::of(status)), None] {
                    if let RunEnd::Failed(failure) = finished(end, text) {
                        assert!(CODES.contains(&failure.code()), "{status:?}: {}", failure.code());
                        assert!(!failure.code().contains("private"));
                    }
                }
            }
        }
        let unknown = serde_json::json!({"kind": secret, "subtype": secret});
        let RunEnd::Failed(failure) = finished(Some(TurnEnd::of_json(&unknown)), None) else { panic!("unknown kinds fail closed") };
        assert_eq!(failure.code(), "error");
    }
}
