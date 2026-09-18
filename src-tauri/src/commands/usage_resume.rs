//! Usage-limit auto-resume.
//!
//! When a provider stops a run because the account's subscription usage
//! limit is hit, the thread gets a [`ProviderRuntimeEvent::UsageLimitReached`]
//! notice and — when the provider reported when the limit lifts — an
//! automatic resume scheduled shortly after that moment.
//!
//! Principles:
//!
//! * **Only a reported reset is acted on.** No reset time is ever invented;
//!   an unknown reset leaves the notice with a manual "Resume now" only.
//! * **The backend owns the schedule.** One scheduler task per app, so
//!   several windows or web-remote clients watching the same thread can
//!   never double-fire.
//! * **Durable.** Pending resumes live in `agent_chat_usage_resumes`, so a
//!   multi-hour wait survives an app restart; rows that came due while the
//!   app was closed fire on the first tick.
//! * **The user wins.** Any user send on the thread forgets the pending
//!   resume (and resets the attempt counter); a turn found in flight at fire
//!   time disarms it.
//! * **Capped.** At most [`MAX_AUTO_ATTEMPTS`] automatic resumes since the
//!   user's last own message.
//! * **One code path.** The manual resume command and the scheduler send the
//!   exact same text through the exact same send path as a user turn.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use tauri::{AppHandle, Manager, Runtime, State};

use crate::agent_provider::{ProviderKind, ProviderRuntimeEvent, ThreadId};
use crate::commands::usage::PlanQuotaStore;
use crate::database::DatabaseStore;
use crate::observability::ObservabilityStore;

use super::agent_chat::{
    feature_flag_on, forward_event, lookup_provider, send_turn_with_origin, ProviderRegistry,
    SendTurnCommandInput, SubagentTracker, TurnOrigin,
};

/// Settings key gating automatic resumes. Absent or anything but `"false"`
/// means enabled — the feature defaults ON.
pub const AUTO_RESUME_SETTING_KEY: &str = "agents.auto_resume_usage_limit";

/// Added after the reported reset before resuming. Resuming a hair early
/// would spend an attempt on the same wall; the margin also absorbs clock
/// skew between this machine and the provider and the scheduler's coarse
/// tick.
pub const RESUME_GRACE_MS: i64 = 45_000;

/// A reset further out than this is reported to the user but not armed —
/// nobody wants a thread to spring back to life days later.
pub const MAX_RESUME_WAIT_MS: i64 = 26 * 60 * 60 * 1000;

/// Automatic resumes allowed since the user's last own message.
pub const MAX_AUTO_ATTEMPTS: i64 = 2;

/// How often the scheduler looks for due resumes. Coarse on purpose: the
/// grace period absorbs the skew.
const SCHEDULER_TICK: Duration = Duration::from_secs(15);

/// Leading text of every automatically built resume turn.
///
/// The frontend matches on this prefix to render the resumed turn compactly
/// instead of as a wall of repeated request text, and the resume-text
/// builder skips user rows that start with it so a second resume re-sends
/// the user's own request rather than the previous note. Changing it
/// breaks both.
pub const USAGE_RESUME_NOTE_PREFIX: &str =
    "[Resumed automatically after a provider usage limit reset";

const USAGE_RESUME_NOTE: &str = "[Resumed automatically after a provider usage limit reset. \
Some of the work may already be complete — check the current state before redoing anything, \
then finish the task. The interrupted request was:]";

/// Longest slice of the interrupted request quoted back, in characters.
const MAX_QUOTED_REQUEST_CHARS: usize = 8000;

/// Sent when the thread has no prior user message to quote.
const BARE_CONTINUE_TEXT: &str = "Continue.";

/// Sent instead of a quoted request when the interrupted request was a goal
/// command: the provider's goal loop resumes itself.
const GOAL_RESUME_TEXT: &str = "/goal resume";

/// When an automatic resume should fire, or `None` for "do not arm".
///
/// Pure so the whole policy is unit-testable:
/// * disabled by the user → not armed;
/// * reset unknown, or already in the past → not armed (nothing reported to
///   wait for);
/// * reset more than [`MAX_RESUME_WAIT_MS`] out → not armed;
/// * [`MAX_AUTO_ATTEMPTS`] already spent → not armed;
/// * otherwise the reported reset plus [`RESUME_GRACE_MS`].
pub fn plan_auto_resume(
    now_ms: i64,
    resets_at_ms: Option<i64>,
    enabled: bool,
    attempts_so_far: i64,
) -> Option<i64> {
    if !enabled || attempts_so_far >= MAX_AUTO_ATTEMPTS {
        return None;
    }
    let reset = resets_at_ms?;
    if reset <= now_ms || reset - now_ms > MAX_RESUME_WAIT_MS {
        return None;
    }
    Some(reset + RESUME_GRACE_MS)
}

/// Whether `text` is a `/goal` command (`^/goal(\s|$)`, case-insensitive,
/// after trimming).
fn is_goal_command(text: &str) -> bool {
    let trimmed = text.trim();
    let Some(head) = trimmed.get(..5) else {
        return false;
    };
    head.eq_ignore_ascii_case("/goal")
        && trimmed[5..]
            .chars()
            .next()
            .is_none_or(char::is_whitespace)
}

/// The text a usage-limit resume sends, built from the thread's most recent
/// user message that is not itself a resume note.
pub fn build_usage_resume_text(last_user_text: Option<&str>) -> String {
    let Some(original) = last_user_text.filter(|t| !t.trim().is_empty()) else {
        return BARE_CONTINUE_TEXT.to_string();
    };
    if is_goal_command(original) {
        return GOAL_RESUME_TEXT.to_string();
    }
    let quoted = if original.chars().count() > MAX_QUOTED_REQUEST_CHARS {
        let mut clipped: String = original.chars().take(MAX_QUOTED_REQUEST_CHARS).collect();
        clipped.push('…');
        clipped
    } else {
        original.to_string()
    };
    format!("{USAGE_RESUME_NOTE}\n\n{quoted}")
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn provider_id(provider: ProviderKind) -> String {
    serde_json::to_value(provider)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{provider:?}").to_lowercase())
}

fn parse_provider(id: &str) -> Option<ProviderKind> {
    serde_json::from_value(serde_json::Value::String(id.to_string())).ok()
}

fn auto_resume_enabled(db: &DatabaseStore) -> bool {
    db.get_setting(AUTO_RESUME_SETTING_KEY).as_deref() != Some("false")
}

// ── Per-turn dedupe ──────────────────────────────────────────────────

/// Which threads already produced a usage-limit notice in their current
/// turn, and the reset it reported.
///
/// A provider can signal the same limit twice (Claude sends a rejected
/// rate-limit reading, and some providers also end the turn with a
/// `rate_limit` error status), so the notice is emitted once per turn. A
/// later signal passes only when it reports a *later* reset — that is a
/// more accurate wait (e.g. the weekly window behind the 5-hour one) and
/// re-arms the resume.
#[derive(Default)]
struct TurnNotices {
    by_thread: HashMap<String, Option<i64>>,
}

impl TurnNotices {
    /// Record a notice; `false` means it duplicates one already emitted this
    /// turn and must be dropped.
    fn admit(&mut self, thread_id: &str, resets_at_ms: Option<i64>) -> bool {
        match self.by_thread.get(thread_id) {
            Some(prev) if resets_at_ms <= *prev => false,
            _ => {
                self.by_thread.insert(thread_id.to_string(), resets_at_ms);
                true
            }
        }
    }

    fn contains(&self, thread_id: &str) -> bool {
        self.by_thread.contains_key(thread_id)
    }

    fn end_turn(&mut self, thread_id: &str) {
        self.by_thread.remove(thread_id);
    }
}

fn turn_notices() -> &'static Mutex<TurnNotices> {
    static NOTICES: OnceLock<Mutex<TurnNotices>> = OnceLock::new();
    NOTICES.get_or_init(|| Mutex::new(TurnNotices::default()))
}

// ── forward_event hooks ──────────────────────────────────────────────

/// Build the `UsageLimitReached` to emit ahead of a `TurnCompleted` whose
/// status is the `rate_limit` error, for providers that signal the limit
/// only through the turn status. The reset comes from the provider's last
/// quota reading (earliest future reset of an exhausted window), or stays
/// unknown. `None` when the thread already got a notice this turn or its
/// provider cannot be resolved.
pub(super) fn synthesize_for_rate_limited_turn<R: Runtime>(
    app: &AppHandle<R>,
    thread_id: &ThreadId,
) -> Option<ProviderRuntimeEvent> {
    if turn_notices()
        .lock()
        .map(|n| n.contains(&thread_id.0))
        .unwrap_or(false)
    {
        return None;
    }
    let provider = {
        let db: State<'_, DatabaseStore> = app.state();
        db.get_agent_chat_session(&thread_id.0)
            .and_then(|record| parse_provider(&record.provider))?
    };
    let fallback = app
        .try_state::<PlanQuotaStore>()
        .and_then(|quota| quota.exhausted_reset_for(&provider_id(provider), now_ms()));
    Some(ProviderRuntimeEvent::UsageLimitReached {
        thread_id: thread_id.clone(),
        provider,
        resets_at_ms: fallback.as_ref().map(|(at, _)| *at),
        auto_resume_at_ms: None,
        window: fallback.and_then(|(_, label)| label),
    })
}

/// The turn is over: the next turn may raise its own notice.
pub(super) fn end_turn(thread_id: &ThreadId) {
    if let Ok(mut notices) = turn_notices().lock() {
        notices.end_turn(&thread_id.0);
    }
}

/// Central arming step for a `UsageLimitReached` passing through
/// `forward_event`: dedupe, decide, persist the schedule, and fill in
/// `auto_resume_at_ms` before the event is persisted and fanned out.
///
/// Returns `false` when the event duplicates this turn's notice and must be
/// dropped entirely.
pub(super) fn arm_usage_limit_event<R: Runtime>(
    app: &AppHandle<R>,
    event: &mut ProviderRuntimeEvent,
) -> bool {
    let ProviderRuntimeEvent::UsageLimitReached {
        thread_id,
        provider,
        resets_at_ms,
        auto_resume_at_ms,
        ..
    } = event
    else {
        return true;
    };
    let admitted = turn_notices()
        .lock()
        .map(|mut n| n.admit(&thread_id.0, *resets_at_ms))
        .unwrap_or(true);
    if !admitted {
        return false;
    }

    let db: State<'_, DatabaseStore> = app.state();
    let existing = db.get_agent_chat_usage_resume(&thread_id.0);
    let attempts = existing.as_ref().map(|row| row.attempts).unwrap_or(0);
    let plan = plan_auto_resume(now_ms(), *resets_at_ms, auto_resume_enabled(&db), attempts);
    // Only claim an armed resume once the schedule is durably stored; a row
    // is written for a disarmed plan only when one already exists, so its
    // attempt count is kept.
    *auto_resume_at_ms = if plan.is_some() || existing.is_some() {
        match db.upsert_agent_chat_usage_resume(&thread_id.0, &provider_id(*provider), plan) {
            Ok(()) => plan,
            Err(error) => {
                eprintln!(
                    "[codemux::usage_resume] failed to store resume for thread={}: {error}",
                    thread_id.0
                );
                None
            }
        }
    } else {
        None
    };
    true
}

/// The user sent a turn on this thread: forget any pending resume and reset
/// the attempt count. No event — the frontend clears its countdown on the
/// new user message itself.
pub(super) fn forget_on_user_activity(db: &DatabaseStore, thread_id: &str) {
    if let Err(error) = db.delete_agent_chat_usage_resume(thread_id) {
        eprintln!("[codemux::usage_resume] failed to clear resume for thread={thread_id}: {error}");
    }
}

// ── Dispatch ─────────────────────────────────────────────────────────

/// Send the resume turn through the same path as a user send. The origin
/// tells that path not to treat this as user activity.
async fn dispatch_resume<R: Runtime>(
    app: &AppHandle<R>,
    provider: ProviderKind,
    thread_id: &ThreadId,
) -> Result<(), String> {
    let text = {
        let db: State<'_, DatabaseStore> = app.state();
        let last = db.latest_agent_chat_user_message_text(&thread_id.0, USAGE_RESUME_NOTE_PREFIX);
        build_usage_resume_text(last.as_deref())
    };
    let input = SendTurnCommandInput {
        thread_id: thread_id.clone(),
        text,
        display_text: None,
        skill_ids: Vec::new(),
        skill_text: None,
        include_plugins: true,
        images: Vec::new(),
        model_override: None,
        effort_override: None,
        permission_mode_override: None,
        client_nonce: None,
    };
    send_turn_with_origin(app.clone(), provider, input, TurnOrigin::UsageResume)
        .await
        .map(|_| ())
}

/// Whether the thread has a run in flight — the same two signals
/// `agent_chat_turn_active` ORs.
async fn thread_busy<R: Runtime>(
    app: &AppHandle<R>,
    provider: ProviderKind,
    thread_id: &ThreadId,
) -> bool {
    let registry: State<'_, ProviderRegistry> = app.state();
    let Ok(impl_) = lookup_provider(&registry, provider).await else {
        return false;
    };
    if impl_.turn_active(thread_id).await {
        return true;
    }
    let tracker: State<'_, SubagentTracker> = app.state();
    tracker.delegated_work_holding_turn(&thread_id.0)
}

fn emit_cancelled<R: Runtime>(app: &AppHandle<R>, thread_id: &ThreadId) {
    forward_event(
        app,
        ProviderRuntimeEvent::UsageResumeCancelled {
            thread_id: thread_id.clone(),
        },
    );
}

/// One scheduler pass: fire every due resume.
async fn fire_due_resumes<R: Runtime>(app: &AppHandle<R>) {
    let due = {
        let db: State<'_, DatabaseStore> = app.state();
        db.list_due_agent_chat_usage_resumes(now_ms())
    };
    for row in due {
        let thread_id = ThreadId(row.thread_id.clone());
        let Some(provider) = parse_provider(&row.provider) else {
            let db: State<'_, DatabaseStore> = app.state();
            let _ = db.delete_agent_chat_usage_resume(&row.thread_id);
            emit_cancelled(app, &thread_id);
            continue;
        };
        let enabled = {
            let db: State<'_, DatabaseStore> = app.state();
            auto_resume_enabled(&db)
        };
        if !enabled {
            // Turned off after this resume was armed: disarm, keep attempts.
            let db: State<'_, DatabaseStore> = app.state();
            let _ = db.upsert_agent_chat_usage_resume(&row.thread_id, &row.provider, None);
            emit_cancelled(app, &thread_id);
            continue;
        }
        if thread_busy(app, provider, &thread_id).await {
            // The user is already driving the thread; step aside.
            let db: State<'_, DatabaseStore> = app.state();
            let _ = db.delete_agent_chat_usage_resume(&row.thread_id);
            emit_cancelled(app, &thread_id);
            continue;
        }
        // Mark fired BEFORE dispatching, so a crash mid-dispatch can never
        // fire the same resume twice. `false` = disarmed meanwhile.
        let claimed = {
            let db: State<'_, DatabaseStore> = app.state();
            db.mark_agent_chat_usage_resume_fired(&row.thread_id)
        };
        match claimed {
            Ok(true) => {}
            Ok(false) => continue,
            Err(error) => {
                eprintln!("[codemux::usage_resume] {error}");
                continue;
            }
        }
        eprintln!(
            "[codemux::usage_resume] resuming thread={} provider={provider:?} (attempt {})",
            row.thread_id,
            row.attempts + 1
        );
        if let Err(error) = dispatch_resume(app, provider, &thread_id).await {
            eprintln!(
                "[codemux::usage_resume] automatic resume failed for thread={}: {error}",
                row.thread_id
            );
            emit_cancelled(app, &thread_id);
        }
    }
}

/// Start the single usage-limit resume scheduler. Called once from app
/// setup, next to the stall watchdog.
pub async fn spawn_usage_resume_scheduler<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(SCHEDULER_TICK);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            // The first tick completes immediately, so resumes that came due
            // while the app was closed fire right after startup.
            interval.tick().await;
            fire_due_resumes(&app).await;
        }
    });
}

// ── Commands ─────────────────────────────────────────────────────────

/// "Resume now": send the resume turn immediately. User-initiated, so on
/// success the thread's pending resume is forgotten and its attempt count
/// reset, exactly like a user send.
#[tauri::command]
pub async fn agent_chat_resume_after_usage_limit<R: Runtime>(
    app: AppHandle<R>,
    provider: ProviderKind,
    thread_id: ThreadId,
) -> Result<(), String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;
    // Disarm first so the scheduler cannot fire the same resume while this
    // dispatch is in progress.
    let was_armed = {
        let db: State<'_, DatabaseStore> = app.state();
        match db.get_agent_chat_usage_resume(&thread_id.0) {
            Some(row) if row.resume_at_ms.is_some() => {
                let _ = db.upsert_agent_chat_usage_resume(&thread_id.0, &row.provider, None);
                true
            }
            _ => false,
        }
    };
    match dispatch_resume(&app, provider, &thread_id).await {
        Ok(()) => {
            let db: State<'_, DatabaseStore> = app.state();
            forget_on_user_activity(&db, &thread_id.0);
            Ok(())
        }
        Err(error) => {
            if was_armed {
                emit_cancelled(&app, &thread_id);
            }
            Err(error)
        }
    }
}

/// Disarm a pending automatic resume, keeping its attempt count. Persists
/// and fans out `UsageResumeCancelled` so every client drops the countdown.
#[tauri::command]
pub async fn agent_chat_cancel_usage_resume<R: Runtime>(
    app: AppHandle<R>,
    provider: ProviderKind,
    thread_id: ThreadId,
) -> Result<(), String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;
    {
        let db: State<'_, DatabaseStore> = app.state();
        if db.get_agent_chat_usage_resume(&thread_id.0).is_some() {
            db.upsert_agent_chat_usage_resume(&thread_id.0, &provider_id(provider), None)?;
        }
    }
    emit_cancelled(&app, &thread_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;
    const HOUR: i64 = 60 * 60 * 1000;

    #[test]
    fn plan_arms_at_reported_reset_plus_grace() {
        assert_eq!(
            plan_auto_resume(NOW, Some(NOW + HOUR), true, 0),
            Some(NOW + HOUR + RESUME_GRACE_MS)
        );
        assert_eq!(
            plan_auto_resume(NOW, Some(NOW + HOUR), true, MAX_AUTO_ATTEMPTS - 1),
            Some(NOW + HOUR + RESUME_GRACE_MS)
        );
    }

    #[test]
    fn plan_refuses_when_disabled() {
        assert_eq!(plan_auto_resume(NOW, Some(NOW + HOUR), false, 0), None);
    }

    #[test]
    fn plan_refuses_unknown_or_past_reset() {
        assert_eq!(plan_auto_resume(NOW, None, true, 0), None);
        assert_eq!(plan_auto_resume(NOW, Some(NOW), true, 0), None);
        assert_eq!(plan_auto_resume(NOW, Some(NOW - 1), true, 0), None);
    }

    #[test]
    fn plan_refuses_reset_beyond_max_wait() {
        assert!(plan_auto_resume(NOW, Some(NOW + 26 * HOUR), true, 0).is_some());
        assert_eq!(plan_auto_resume(NOW, Some(NOW + 26 * HOUR + 1), true, 0), None);
        assert_eq!(plan_auto_resume(NOW, Some(NOW + 7 * 24 * HOUR), true, 0), None);
    }

    #[test]
    fn plan_refuses_once_attempts_are_exhausted() {
        assert_eq!(
            plan_auto_resume(NOW, Some(NOW + HOUR), true, MAX_AUTO_ATTEMPTS),
            None
        );
        assert_eq!(plan_auto_resume(NOW, Some(NOW + HOUR), true, 99), None);
    }

    #[test]
    fn resume_text_quotes_the_interrupted_request() {
        let text = build_usage_resume_text(Some("Refactor the parser"));
        assert!(text.starts_with(USAGE_RESUME_NOTE_PREFIX));
        assert!(text.ends_with("The interrupted request was:]\n\nRefactor the parser"));
    }

    #[test]
    fn resume_text_resumes_a_goal_loop_natively() {
        for goal in ["/goal ship it", "  /GOAL  ship it ", "/goal", "/Goal\nmultiline"] {
            assert_eq!(build_usage_resume_text(Some(goal)), "/goal resume", "for {goal:?}");
        }
        // Only the exact command, not a lookalike.
        assert!(build_usage_resume_text(Some("/goals are nice")).starts_with(USAGE_RESUME_NOTE_PREFIX));
        assert!(build_usage_resume_text(Some("please /goal x")).starts_with(USAGE_RESUME_NOTE_PREFIX));
    }

    #[test]
    fn resume_text_truncates_long_requests() {
        let long = "é".repeat(MAX_QUOTED_REQUEST_CHARS + 50);
        let text = build_usage_resume_text(Some(&long));
        let quoted = text.split_once("\n\n").unwrap().1;
        assert_eq!(quoted.chars().count(), MAX_QUOTED_REQUEST_CHARS + 1);
        assert!(quoted.ends_with('…'));

        let exact = "a".repeat(MAX_QUOTED_REQUEST_CHARS);
        assert!(build_usage_resume_text(Some(&exact)).ends_with(&exact));
    }

    #[test]
    fn resume_text_without_a_prior_message_just_continues() {
        assert_eq!(build_usage_resume_text(None), "Continue.");
        assert_eq!(build_usage_resume_text(Some("   ")), "Continue.");
    }

    #[test]
    fn turn_notices_dedupe_per_turn_but_admit_a_later_reset() {
        let mut n = TurnNotices::default();
        assert!(n.admit("t", Some(100)));
        assert!(!n.admit("t", Some(100)), "same reset twice in one turn");
        assert!(!n.admit("t", None), "an unknown reset adds nothing");
        assert!(n.admit("t", Some(200)), "a later reset is more accurate");
        assert!(n.contains("t"));
        assert!(n.admit("other", None), "threads are independent");
        n.end_turn("t");
        assert!(!n.contains("t"));
        assert!(n.admit("t", Some(50)), "a new turn starts fresh");
    }

    #[test]
    fn provider_ids_round_trip() {
        for kind in [
            ProviderKind::Claude,
            ProviderKind::Codex,
            ProviderKind::Cursor,
            ProviderKind::Grok,
            ProviderKind::OpenCode,
        ] {
            assert_eq!(parse_provider(&provider_id(kind)), Some(kind));
        }
        assert_eq!(parse_provider("nope"), None);
    }
}
