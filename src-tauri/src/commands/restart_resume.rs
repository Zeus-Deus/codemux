//! Resume after a restart: chats whose turn was running when the app
//! stopped continue on their own when it opens again.
//!
//! * **Recorded live.** [`on_event`] (called from `forward_event`) keeps
//!   `agent_chat_restart_resumes` (see [`crate::database::restart_resumes`])
//!   listing every running turn and what it waits on. Quitting stops the
//!   bookkeeping ([`super::queued_turns::exiting`]), so the sessions closing
//!   on the way out cannot erase it, and [`on_exit`] stamps the rows as a
//!   normal quit. A crash leaves the rows unstamped. Either way the next
//!   launch finds rows from an earlier run.
//! * **Taken once.** [`spawn_restart_resume`] runs once the providers are
//!   registered. It removes every earlier run's row in one step and decides
//!   each on its own, so a chat is resumed at most once per launch, and a
//!   crash while resuming loses the resume rather than doubling it.
//! * **Checked, then sent.** A chat waiting on an approval or question is
//!   surfaced for the user instead. A stopped chat has no row (stopping
//!   clears it). Closed workspaces, missing worktrees, chats on another
//!   device, delegated children and chats already running are skipped, as
//!   are chats whose conversation can't be reopened as it was: only Claude
//!   and Codex refuse a lost conversation instead of starting a fresh one,
//!   so the other providers keep the manual Continue. Resumes in a row
//!   are capped, so a turn that keeps taking the app down cannot loop.
//!   The rest get one short turn through the normal send path telling the
//!   agent to check the current state before it continues.
//! * **The saved queue follows.** Once the resumed turn finishes cleanly,
//!   the chat's saved queue (`commands::queued_turns`) is sent in order
//!   through the same command the user's Send uses. Chats that were idle
//!   at quit keep their saved queue manual.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::agent_provider::types::MessageDelivery;
use crate::agent_provider::{ProviderKind, ProviderRuntimeEvent, SessionStatus, ThreadId, TurnStatus};
use crate::database::restart_resumes::RestartResumeRecord;
use crate::database::DatabaseStore;
use crate::state::{AppStateStore, PaneStatus};

use super::agent_chat::{lookup_provider, send_turn_with_origin, ProviderRegistry, TurnOrigin};
use super::usage_resume::{activity_lock, now_ms, queued_turn, thread_busy};

/// Settings key gating the resume. Absent or anything but `"false"` means
/// enabled — the feature defaults ON. Off leaves the manual Continue.
pub const RESTART_RESUME_SETTING_KEY: &str = "agents.auto_resume_after_restart";

/// Leading text of every resume turn. The frontend matches on it to show a
/// "Resumed after restart" divider instead of the text. Changing it breaks
/// that.
pub const RESTART_RESUME_NOTE_PREFIX: &str = "[Resumed automatically after an app restart";

/// Automatic resumes allowed in a row before the user's next message.
pub const MAX_RESUMES_IN_A_ROW: i64 = 2;

/// The turn sent to a resumed chat. `graceful` says whether the app quit
/// or stopped unexpectedly.
pub fn resume_note(graceful: bool) -> String {
    let cause = if graceful {
        "Codemux quit"
    } else {
        "Codemux stopped unexpectedly"
    };
    format!(
        "{RESTART_RESUME_NOTE_PREFIX}. {cause} while you were working on this task, so your \
last turn was cut off. Before doing anything else, check the current state: run git status, \
look at the files you were changing, check whether processes or servers you started are still \
running, and re-read your last tool results. Do not redo work that already finished. Then \
continue the task. If you need an approval or an answer from the user, ask again.]"
    )
}

fn enabled(db: &DatabaseStore) -> bool {
    db.get_setting(RESTART_RESUME_SETTING_KEY).as_deref() != Some("false")
}

fn parse_provider(id: &str) -> Option<ProviderKind> {
    serde_json::from_value(serde_json::Value::String(id.to_string())).ok()
}

// ── Live bookkeeping ─────────────────────────────────────────────────

#[derive(Default)]
struct Live {
    /// Automatic resumes in a row behind a thread's current turn. Cleared by
    /// the user's next message.
    chain: HashMap<String, i64>,
    /// Threads resumed this run whose resume turn has not ended yet.
    resuming: HashSet<String>,
    /// Chats the launch pass has taken but not decided yet. The user acting
    /// on one first (a send, a Stop) takes it out of the pass.
    candidates: HashSet<String>,
    /// Saved-queue drains under way, by thread, with the token each was
    /// started under. A Stop removes the entry, which ends the drain before
    /// its next send.
    draining: HashMap<String, u64>,
    next_drain: u64,
}

fn live() -> std::sync::MutexGuard<'static, Live> {
    static LIVE: OnceLock<Mutex<Live>> = OnceLock::new();
    LIVE.get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Mirror one provider event into the running-turn table. Best-effort: a
/// failed write only affects what a restart can resume.
pub(crate) fn on_event<R: Runtime>(app: &AppHandle<R>, event: &ProviderRuntimeEvent) {
    if super::queued_turns::exiting() {
        return;
    }
    let Some(db) = app.try_state::<DatabaseStore>() else {
        return;
    };
    let waiting = |thread_id: &ThreadId, id: &str, open: bool| {
        db.set_restart_turn_waiting(&thread_id.0, id, open)
    };
    let result = match event {
        ProviderRuntimeEvent::SessionStateChanged { thread_id, status } => match status {
            SessionStatus::Running { active_turn } => {
                let chain = live().chain.get(&thread_id.0).copied().unwrap_or(0);
                db.mark_restart_turn_running(
                    &thread_id.0,
                    &active_turn.0,
                    super::queued_turns::epoch(),
                    chain,
                    now_ms(),
                )
            }
            SessionStatus::WaitingApproval { request_id } => waiting(thread_id, &request_id.0, true),
            SessionStatus::Closed | SessionStatus::Error { .. } => {
                db.clear_restart_turn(&thread_id.0).map(drop)
            }
            // `Ready` alone is no end: a session can report it while its
            // first turn is already starting. But a send that fails after
            // `Running` ends with only `Ready`, so check whether a turn is
            // still active and clear the row if not.
            SessionStatus::Ready => {
                settle_if_idle(app, thread_id);
                return;
            }
            SessionStatus::Starting => return,
        },
        ProviderRuntimeEvent::TurnCompleted { thread_id, status, .. } => {
            let result = db.clear_restart_turn(&thread_id.0).map(drop);
            let drain = {
                let mut live = live();
                let resumed = live.resuming.remove(&thread_id.0);
                // Registered before the drain starts, so a Stop from here on
                // always finds it.
                (resumed && matches!(status, TurnStatus::Success)).then(|| {
                    live.next_drain += 1;
                    let token = live.next_drain;
                    live.draining.insert(thread_id.0.clone(), token);
                    token
                })
            };
            if let Some(token) = drain {
                let app = app.clone();
                let thread_id = thread_id.clone();
                tauri::async_runtime::spawn(async move {
                    drain_saved_queue(&app, &thread_id, token).await
                });
            }
            result
        }
        // The provider could not reopen the conversation and started a
        // fresh one: the resumed turn has no history behind it, so its end
        // must not send the saved queue on top.
        ProviderRuntimeEvent::ResumeCursorUpdated { thread_id, resume_cursor }
            if resume_cursor.is_null() =>
        {
            if live().resuming.remove(&thread_id.0) {
                eprintln!(
                    "[codemux::restart_resume] thread={} resumed without its conversation; \
                     saved queue stays manual",
                    thread_id.0
                );
            }
            return;
        }
        ProviderRuntimeEvent::RequestOpened { thread_id, request_id, .. } => {
            waiting(thread_id, &request_id.0, true)
        }
        ProviderRuntimeEvent::RequestResolved { thread_id, request_id, .. }
        | ProviderRuntimeEvent::RequestResponseFailed { thread_id, request_id, .. } => {
            waiting(thread_id, &request_id.0, false)
        }
        ProviderRuntimeEvent::QuestionsAsked { thread_id, question } => {
            waiting(thread_id, &question.id, true)
        }
        ProviderRuntimeEvent::QuestionResolved { thread_id, question_id, .. } => {
            waiting(thread_id, question_id, false)
        }
        _ => return,
    };
    if let Err(error) = result {
        eprintln!("[codemux::restart_resume] {error}");
    }
}

/// Clear the thread's running turn, unless a turn is still active or a newer
/// one replaced it meanwhile.
fn settle_if_idle<R: Runtime>(app: &AppHandle<R>, thread_id: &ThreadId) {
    let Some(db) = app.try_state::<DatabaseStore>() else {
        return;
    };
    let Some(row) = db.get_restart_turn(&thread_id.0) else {
        return;
    };
    let Some(provider) = db
        .get_agent_chat_session(&thread_id.0)
        .and_then(|session| parse_provider(&session.provider))
    else {
        return;
    };
    let app = app.clone();
    let thread_id = thread_id.clone();
    tauri::async_runtime::spawn(async move {
        if super::queued_turns::exiting() || thread_busy(&app, provider, &thread_id).await {
            return;
        }
        let db: State<'_, DatabaseStore> = app.state();
        if let Err(error) = db.clear_restart_turn_if(&thread_id.0, &row.turn_id) {
            eprintln!("[codemux::restart_resume] {error}");
        }
    });
}

/// The user sent a message: automatic resumes start counting again, and a
/// chat still waiting for the launch pass is the user's now.
pub(super) fn on_user_activity(thread_id: &str) {
    let mut live = live();
    live.chain.remove(thread_id);
    live.candidates.remove(thread_id);
}

/// The user stopped or closed the chat: nothing to resume, and its saved
/// queue stays manual.
pub(super) fn forget_stopped_thread<R: Runtime>(app: &AppHandle<R>, thread_id: &str) {
    if super::queued_turns::exiting() {
        return;
    }
    {
        let mut live = live();
        live.resuming.remove(thread_id);
        live.chain.remove(thread_id);
        live.candidates.remove(thread_id);
        live.draining.remove(thread_id);
    }
    if let Some(db) = app.try_state::<DatabaseStore>() {
        if let Err(error) = db.clear_restart_turn(thread_id) {
            eprintln!("[codemux::restart_resume] {error}");
        }
    }
}

/// The app is quitting: mark this run's running turns as stopped by a quit.
/// Call after [`super::queued_turns::mark_exiting`].
pub fn on_exit<R: Runtime>(app: &AppHandle<R>) {
    let Some(db) = app.try_state::<DatabaseStore>() else {
        return;
    };
    match db.stamp_restart_turns_quit(super::queued_turns::epoch(), now_ms()) {
        Ok(0) => {}
        Ok(count) => eprintln!("[codemux::restart_resume] {count} running chat(s) will resume next launch"),
        Err(error) => eprintln!("[codemux::restart_resume] {error}"),
    }
}

// ── Deciding ─────────────────────────────────────────────────────────

/// What is known about a chat an earlier run left running.
#[derive(Debug, Clone, Default)]
pub struct ResumeFacts {
    pub enabled: bool,
    /// Open approvals or questions when the app stopped.
    pub waiting: bool,
    /// Automatic resumes in a row that led to the cut-off turn.
    pub chain: i64,
    pub session_exists: bool,
    /// The workspace is open (closed workspaces are archived).
    pub workspace_open: bool,
    /// The chat runs here, not on a remote host or another device.
    pub local: bool,
    pub worktree_exists: bool,
    pub delegated_child: bool,
    /// The provider can reopen this conversation with its history, and
    /// refuses rather than starting over when it can't.
    pub provider_resumes: bool,
    /// Something is already running on the chat, or a usage-limit resume
    /// owns it.
    pub busy: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    Disabled,
    NoSession,
    WorkspaceClosed,
    NotLocal,
    WorktreeMissing,
    DelegatedChild,
    ManualOnly,
    CapReached,
    Busy,
    /// The provider could not reopen the original conversation.
    ConversationLost,
    /// The user acted on the chat before the launch pass reached it.
    UserTookOver,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeDecision {
    Resume,
    /// Blocked on the user: show it as needing input, do not send.
    Resurface,
    Skip(SkipReason),
}

/// Pure, so every rule is unit-testable.
pub fn decide(facts: &ResumeFacts) -> ResumeDecision {
    use ResumeDecision::*;
    use SkipReason::*;
    if !facts.enabled {
        return Skip(Disabled);
    }
    if !facts.session_exists {
        return Skip(NoSession);
    }
    if !facts.workspace_open {
        return Skip(WorkspaceClosed);
    }
    if !facts.local {
        return Skip(NotLocal);
    }
    if !facts.worktree_exists {
        return Skip(WorktreeMissing);
    }
    if facts.delegated_child {
        return Skip(DelegatedChild);
    }
    if facts.waiting {
        return Resurface;
    }
    if facts.busy {
        return Skip(Busy);
    }
    if facts.chain >= MAX_RESUMES_IN_A_ROW {
        return Skip(CapReached);
    }
    if !facts.provider_resumes {
        return Skip(ManualOnly);
    }
    Resume
}

async fn gather_facts<R: Runtime>(
    app: &AppHandle<R>,
    row: &RestartResumeRecord,
) -> (ResumeFacts, Option<ProviderKind>) {
    let db: State<'_, DatabaseStore> = app.state();
    let state: State<'_, AppStateStore> = app.state();
    let mut facts = ResumeFacts {
        enabled: enabled(&db),
        waiting: !row.waiting.is_empty(),
        chain: row.chain,
        ..Default::default()
    };
    let Some(session) = db.get_agent_chat_session(&row.thread_id) else {
        return (facts, None);
    };
    facts.session_exists = true;
    let provider = parse_provider(&session.provider);
    if let Some(workspace) = state.find_workspace(&session.workspace_id) {
        facts.workspace_open = true;
        facts.local = workspace.host_id.is_none()
            && !workspace.attach_only
            && !super::agent_chat::thread_runs_on_device_pub(app, &row.thread_id);
        let dir = session
            .cwd
            .clone()
            .or(workspace.worktree_path.clone())
            .unwrap_or(workspace.cwd.clone());
        facts.worktree_exists = std::path::Path::new(&dir).is_dir();
    }
    facts.delegated_child = super::delegation::is_delegated_child(app, &row.thread_id);
    if let Some(provider) = provider {
        let registry: State<'_, ProviderRegistry> = app.state();
        // Only providers that refuse, rather than quietly replace, a
        // conversation they can't reopen (see `dispatch`). The others start
        // a fresh session inside the adapter and stay on the manual
        // Continue.
        let resumable = matches!(provider, ProviderKind::Claude | ProviderKind::Codex)
            && lookup_provider(&registry, provider)
                .await
                .is_ok_and(|impl_| impl_.capabilities().supports_session_resume);
        // Without its saved conversation the provider would start over, and
        // the note would make no sense to it. Claude's lives in a local file
        // the session start would drop the id for.
        let conversation = session.sdk_session_id.as_deref().is_some_and(|id| {
            provider != ProviderKind::Claude
                || !facts.local
                || !super::agent_chat::claude_session_file_missing(id)
        });
        facts.provider_resumes = resumable && conversation && session.imported_from.is_none();
        facts.busy = thread_busy(app, provider, &ThreadId(row.thread_id.clone())).await
            || db
                .get_agent_chat_usage_resume(&row.thread_id)
                .is_some_and(|usage| usage.resume_at_ms.is_some());
    }
    (facts, provider)
}

// ── Acting ───────────────────────────────────────────────────────────

/// Show a chat that was waiting on the user as needing input again.
fn resurface<R: Runtime>(app: &AppHandle<R>, thread_id: &str) {
    let state: State<'_, AppStateStore> = app.state();
    if state.set_pane_status_by_thread(thread_id, PaneStatus::Permission) {
        let snapshot = state.stamped_snapshot();
        let _ = app.emit("app-state-changed", &snapshot);
    }
    if let Some(target) = state.notification_target_for_thread(thread_id) {
        crate::notifications::notify_agent(app, crate::notifications::AgentNotice::NeedsInput, &target);
    }
}

const CONVERSATION_LOST: &str = "the original conversation could not be reopened";

async fn dispatch<R: Runtime>(
    app: &AppHandle<R>,
    provider: ProviderKind,
    row: &RestartResumeRecord,
) -> Result<(), String> {
    let thread_id = ThreadId(row.thread_id.clone());
    // Reopen the original conversation first. The send path would quietly
    // start a fresh one if it can't, and the note means nothing there.
    super::agent_chat::ensure_live_session_mode(app, provider, &thread_id, true)
        .await
        .map_err(|error| format!("{CONVERSATION_LOST}: {error}"))?;
    {
        let mut live = live();
        live.chain.insert(row.thread_id.clone(), row.chain + 1);
        live.resuming.insert(row.thread_id.clone());
    }
    let input = queued_turn(thread_id, resume_note(row.quit_at_ms.is_some()));
    let result = send_turn_with_origin(app.clone(), provider, input, TurnOrigin::RestartResume).await;
    if result.is_err() {
        let mut live = live();
        live.resuming.remove(&row.thread_id);
        live.chain.remove(&row.thread_id);
    }
    result.map(drop)
}

/// Decide and act on one chat an earlier run left running.
async fn resume_one<R: Runtime>(app: &AppHandle<R>, row: RestartResumeRecord) -> ResumeDecision {
    let lock = activity_lock(&row.thread_id);
    // Holding the activity lock makes a user send wait for this dispatch;
    // a send already under way wins.
    let Ok(_guard) = lock.try_lock() else {
        live().candidates.remove(&row.thread_id);
        return ResumeDecision::Skip(SkipReason::Busy);
    };
    // Taken out under the lock: a user send or Stop either ran before this
    // (and removed it) or waits for this decision.
    if !live().candidates.remove(&row.thread_id) {
        return ResumeDecision::Skip(SkipReason::UserTookOver);
    }
    let (facts, provider) = gather_facts(app, &row).await;
    let mut decision = decide(&facts);
    match (decision, provider) {
        (ResumeDecision::Resurface, _) => resurface(app, &row.thread_id),
        (ResumeDecision::Resume, Some(provider)) => {
            eprintln!(
                "[codemux::restart_resume] resuming thread={} provider={provider:?}",
                row.thread_id
            );
            if let Err(error) = dispatch(app, provider, &row).await {
                eprintln!(
                    "[codemux::restart_resume] resume failed for thread={}: {error}",
                    row.thread_id
                );
                if error.starts_with(CONVERSATION_LOST) {
                    decision = ResumeDecision::Skip(SkipReason::ConversationLost);
                }
            }
        }
        (decision, _) => eprintln!(
            "[codemux::restart_resume] not resuming thread={}: {decision:?}",
            row.thread_id
        ),
    }
    decision
}

/// Take every chat a run other than `epoch` left running and resume each
/// that should.
async fn resume_after_restart<R: Runtime>(
    app: &AppHandle<R>,
    epoch: &str,
) -> Vec<(String, ResumeDecision)> {
    let rows = {
        let db: State<'_, DatabaseStore> = app.state();
        match db.take_stale_restart_turns(epoch) {
            Ok(rows) => rows,
            Err(error) => {
                eprintln!("[codemux::restart_resume] {error}");
                return Vec::new();
            }
        }
    };
    live()
        .candidates
        .extend(rows.iter().map(|row| row.thread_id.clone()));
    let mut decisions = Vec::with_capacity(rows.len());
    for row in rows {
        let thread_id = row.thread_id.clone();
        decisions.push((thread_id, resume_one(app, row).await));
    }
    decisions
}

/// Send a resumed chat's saved queue in order, through the same command the
/// user's Send uses. The first starts a turn and the rest queue behind it.
/// A Stop on the chat ends the drain before its next send, and the first
/// failure ends it too; what is left stays saved.
async fn drain_saved_queue<R: Runtime>(app: &AppHandle<R>, thread_id: &ThreadId, token: u64) {
    let cancelled = || live().draining.get(&thread_id.0) != Some(&token);
    let finish = || {
        let mut live = live();
        if live.draining.get(&thread_id.0) == Some(&token) {
            live.draining.remove(&thread_id.0);
        }
    };
    let provider = {
        let db: State<'_, DatabaseStore> = app.state();
        db.get_agent_chat_session(&thread_id.0)
            .and_then(|session| parse_provider(&session.provider))
    };
    let Some(provider) = provider else {
        finish();
        return;
    };
    // Listed through the pane's command, which drops rows whose message
    // already reached the transcript before an earlier crash.
    let saved: Vec<String> =
        match super::queued_turns::agent_chat_list_queued_turns(app.clone(), thread_id.clone()).await {
            Ok(rows) => rows.into_iter().filter(|row| row.saved).map(|row| row.queued_id).collect(),
            Err(error) => {
                eprintln!("[codemux::restart_resume] {error}");
                finish();
                return;
            }
        };
    for queued_id in saved {
        if cancelled() {
            eprintln!(
                "[codemux::restart_resume] saved queue on thread={} stays saved: stopped",
                thread_id.0
            );
            return;
        }
        let sent = super::queued_turns::agent_chat_send_saved_queued_turn(
            app.clone(),
            provider,
            thread_id.clone(),
            queued_id.clone(),
            MessageDelivery::Queue,
        )
        .await;
        if let Err(error) = sent {
            eprintln!(
                "[codemux::restart_resume] saved turn {queued_id} on thread={} stays saved: {error}",
                thread_id.0
            );
            break;
        }
    }
    finish();
}

/// Resume the chats the last run left running. Called once from app setup,
/// after the providers are registered and the delegation journal drained.
pub async fn spawn_restart_resume<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        resume_after_restart(&app, super::queued_turns::epoch()).await;
    });
}

/// Run the launch pass with mock providers, as the launch after this run
/// (`next_epoch`), and return each decision.
#[cfg(feature = "test-fixtures")]
pub async fn resume_after_restart_for_test<R: Runtime>(
    app: &AppHandle<R>,
    next_epoch: &str,
) -> Vec<(String, ResumeDecision)> {
    resume_after_restart(app, next_epoch).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resumable() -> ResumeFacts {
        ResumeFacts {
            enabled: true,
            session_exists: true,
            workspace_open: true,
            local: true,
            worktree_exists: true,
            provider_resumes: true,
            ..Default::default()
        }
    }

    #[test]
    fn a_running_chat_in_an_open_workspace_resumes() {
        assert_eq!(decide(&resumable()), ResumeDecision::Resume);
        let after_one = ResumeFacts { chain: MAX_RESUMES_IN_A_ROW - 1, ..resumable() };
        assert_eq!(decide(&after_one), ResumeDecision::Resume);
    }

    #[test]
    fn a_chat_waiting_on_the_user_is_resurfaced_not_sent() {
        let facts = ResumeFacts { waiting: true, ..resumable() };
        assert_eq!(decide(&facts), ResumeDecision::Resurface);
        // Even when the provider could not resume it on its own.
        let manual = ResumeFacts { provider_resumes: false, ..facts };
        assert_eq!(decide(&manual), ResumeDecision::Resurface);
    }

    #[test]
    fn skip_rules() {
        use SkipReason::*;
        let cases = [
            (ResumeFacts { enabled: false, ..resumable() }, Disabled),
            (ResumeFacts { session_exists: false, ..resumable() }, NoSession),
            (ResumeFacts { workspace_open: false, ..resumable() }, WorkspaceClosed),
            (ResumeFacts { local: false, ..resumable() }, NotLocal),
            (ResumeFacts { worktree_exists: false, ..resumable() }, WorktreeMissing),
            (ResumeFacts { delegated_child: true, ..resumable() }, DelegatedChild),
            (ResumeFacts { busy: true, ..resumable() }, Busy),
            (ResumeFacts { chain: MAX_RESUMES_IN_A_ROW, ..resumable() }, CapReached),
            (ResumeFacts { provider_resumes: false, ..resumable() }, ManualOnly),
        ];
        for (facts, reason) in cases {
            assert_eq!(decide(&facts), ResumeDecision::Skip(reason), "{facts:?}");
        }
        // Turned off: even a waiting chat is left as it is today.
        let off = ResumeFacts { enabled: false, waiting: true, ..resumable() };
        assert_eq!(decide(&off), ResumeDecision::Skip(Disabled));
        // An archived chat waiting on the user is not resurfaced either.
        let archived = ResumeFacts { workspace_open: false, waiting: true, ..resumable() };
        assert_eq!(decide(&archived), ResumeDecision::Skip(WorkspaceClosed));
    }

    #[test]
    fn the_note_carries_the_prefix_and_the_cause() {
        let quit = resume_note(true);
        let crash = resume_note(false);
        assert!(quit.starts_with(RESTART_RESUME_NOTE_PREFIX));
        assert!(crash.starts_with(RESTART_RESUME_NOTE_PREFIX));
        assert!(quit.contains("Codemux quit") && crash.contains("stopped unexpectedly"));
        assert!(quit.contains("git status") && quit.contains("Do not redo work"));
        // The usage-limit resume skips both automatic notes when it quotes
        // the user's request.
        assert!(quit.starts_with(super::super::usage_resume::AUTOMATIC_RESUME_PREFIX));
    }
}
