//! Tests for waiting on native runs: the change signal, the wait cursor and
//! `thread_wait`/`thread_status` against the shared fixtures. They call
//! `super::wait` directly; the registry routes to the same functions.
use super::outcome::RunState;
use super::tests::{connector_caller, connector_off, fixture, mock_agent_provider, native_caller_fixture, settle_operation};
use super::wait::{self, Change};
use super::{execute, ControlAccess, ControlCaller, ControlError, ErrorCode, NativeControlState};
use crate::agent_provider::{
    CompletedItem, ProviderKind, ProviderRuntimeEvent as Event, RequestId, SessionStatus, ThreadId, TurnId, TurnStatus,
    UserQuestion, UserQuestionSet,
};
use crate::commands::agent_chat::forward_event;
use crate::database::DatabaseStore;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tauri::Manager;
use tokio::sync::watch;
use tokio::time::{timeout, Instant};

type App = tauri::App<tauri::test::MockRuntime>;

/// A persisted chat with no live provider session.
fn persisted(app: &App, workspace: &str, thread: &str) {
    app.state::<DatabaseStore>().upsert_agent_chat_session(thread, workspace, None, "claude").unwrap();
}

/// A chat with a pane and a live mock session, as `thread_respond` needs.
async fn launched(app: &App, workspace: &str, key: &str) -> String {
    let receipt = execute(app.handle(), &ControlCaller::trusted(), "thread_launch", json!({
        "workspace_id": workspace, "client_request_id": key, "provider": "claude", "permission_mode": "default",
    })).await.unwrap();
    let launched = settle_operation(app, &receipt).await;
    assert_eq!(launched["state"], "succeeded", "{launched}");
    launched["thread_id"].as_str().unwrap().to_string()
}

fn running(app: &App, thread: &str, turn: &str) {
    forward_event(app.handle(), Event::SessionStateChanged {
        thread_id: ThreadId(thread.into()), status: SessionStatus::Running { active_turn: TurnId(turn.into()) },
    });
}

fn reply(app: &App, thread: &str, turn: &str, text: &str) {
    forward_event(app.handle(), Event::ItemCompleted {
        thread_id: ThreadId(thread.into()), turn_id: TurnId(turn.into()),
        item: CompletedItem::AssistantText { text: text.into() }, subagent_id: None,
    });
}

fn completed(app: &App, thread: &str, turn: &str, status: TurnStatus) {
    forward_event(app.handle(), Event::TurnCompleted {
        thread_id: ThreadId(thread.into()), turn_id: TurnId(turn.into()), status, usage: None,
    });
}

/// A person's message that started `turn`, persisted as the send path does
/// once the provider took it.
fn person_started(app: &App, thread: &str, turn: &str) {
    let message = json!({"type": "user_message", "thread_id": thread, "text": "Next task", "message_id": format!("message-{thread}-{turn}")});
    app.state::<DatabaseStore>().control_append_user_message(thread, &message.to_string(), turn, false).unwrap();
}

fn session(app: &App, thread: &str, status: SessionStatus) {
    forward_event(app.handle(), Event::SessionStateChanged { thread_id: ThreadId(thread.into()), status });
}

fn target(workspace: &str, thread: &str, extra: Value) -> Value {
    let mut args = json!({"workspace_id": workspace, "thread_id": thread});
    args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    args
}

async fn wait_for(app: &App, caller: &ControlCaller, args: Value) -> Result<Value, ControlError> {
    wait::thread_wait(app.handle(), caller, args).await
}

/// Start a wait in the background; it subscribes before the test moves on.
fn spawn_wait(app: &App, caller: &ControlCaller, args: Value) -> tokio::task::JoinHandle<Result<Value, ControlError>> {
    let (handle, caller) = (app.handle().clone(), caller.clone());
    tokio::spawn(async move { wait::thread_wait(&handle, &caller, args).await })
}

async fn parked() {
    tokio::time::sleep(Duration::from_millis(50)).await;
}

const PROMPT: Duration = Duration::from_millis(500);

fn keys(value: &Value, found: &mut Vec<String>) {
    match value {
        Value::Object(map) => for (key, value) in map {
            found.push(key.clone());
            keys(value, found);
        },
        Value::Array(items) => items.iter().for_each(|item| keys(item, found)),
        _ => {}
    }
}

fn private_channel() -> watch::Sender<Change> {
    watch::channel(Change::default()).0
}

// ── Signal and cursor ─────────────────────────────────────────────────────

#[tokio::test]
async fn a_signal_after_subscribe_is_never_lost() {
    let thread = "signal-after-subscribe";
    let mut changes = wait::subscribe(thread);
    wait::signal(Some(thread));
    timeout(Duration::from_millis(10), changes.changed()).await.expect("a signal raised after subscribe wakes the waiter");
    // Raised between subscribe and the first await: still seen.
    let sender = private_channel();
    let mut changes = wait::subscribe_on(&sender, thread);
    wait::signal_on(&sender, Some(thread));
    timeout(Duration::from_millis(10), changes.changed()).await.expect("no change is lost");
}

#[tokio::test]
async fn a_single_signal_for_another_thread_is_skipped() {
    // A private channel: other tests in this process raise the global one.
    let sender = private_channel();
    let mut changes = wait::subscribe_on(&sender, "mine");
    wait::signal_on(&sender, Some("theirs"));
    assert!(timeout(Duration::from_millis(50), changes.changed()).await.is_err(), "one change to another thread is skipped");
    // Two changes read as one generation jump may hide one of ours.
    wait::signal_on(&sender, Some("theirs"));
    wait::signal_on(&sender, Some("theirs"));
    timeout(Duration::from_millis(50), changes.changed()).await.expect("coalesced changes wake the waiter");
    wait::signal_on(&sender, Some("mine"));
    timeout(Duration::from_millis(50), changes.changed()).await.expect("a change to this thread wakes the waiter");
}

#[tokio::test]
async fn signal_all_wakes_every_thread() {
    let sender = private_channel();
    let (mut a, mut b) = (wait::subscribe_on(&sender, "a"), wait::subscribe_on(&sender, "b"));
    wait::signal_on(&sender, None);
    timeout(Duration::from_millis(50), a.changed()).await.expect("a wakes");
    timeout(Duration::from_millis(50), b.changed()).await.expect("b wakes");
    // An empty thread name concerns no thread in particular, so everyone.
    wait::signal_on(&sender, Some(""));
    timeout(Duration::from_millis(50), a.changed()).await.expect("a wakes on an untagged change");
    let mut global = wait::subscribe("signal-all-global");
    wait::signal_all();
    timeout(Duration::from_millis(50), global.changed()).await.expect("signal_all wakes waiters on the shared signal");
}

#[test]
fn wait_cursor_changes_only_with_state_turn_or_pending() {
    let base = wait::cursor(RunState::Running, Some("t1"), false, &[]);
    assert_eq!(base.len(), 16);
    assert!(base.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(base, wait::cursor(RunState::Running, Some("t1"), false, &[]), "the same view gives the same cursor");
    for other in [
        wait::cursor(RunState::NeedsInput, Some("t1"), false, &[]),
        wait::cursor(RunState::Running, Some("t2"), false, &[]),
        wait::cursor(RunState::Running, None, false, &[]),
        wait::cursor(RunState::Running, Some("t1"), true, &[]),
        wait::cursor(RunState::Running, Some("t1"), false, &["a:handle".into()]),
    ] {
        assert_ne!(base, other);
    }
    assert_ne!(
        wait::cursor(RunState::NeedsInput, Some("t1"), false, &["q:one".into()]),
        wait::cursor(RunState::NeedsInput, Some("t1"), false, &["q:two".into()]),
    );
}

/// Transports decide what parks from `ToolSpec::may_wait`; the test helper
/// must name the same tools, or wait tests stop covering what doors budget.
#[test]
fn only_thread_wait_parks() {
    assert!(wait::is_long_wait("thread_wait"));
    for id in super::registry::ToolId::ALL {
        let spec = id.spec();
        assert_eq!(wait::is_long_wait(spec.name()), spec.may_wait(), "{}", spec.name());
    }
}

// ── thread_wait and thread_status ─────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_returns_on_turn_completion_without_polling() {
    let (app, _provider, _root, workspace) = fixture().await;
    let thread = "wait-pong";
    persisted(&app, &workspace, thread);
    running(&app, thread, "t1");
    let waiter = spawn_wait(&app, &ControlCaller::trusted(), target(&workspace, thread, json!({"timeout_ms": 5000})));
    parked().await;
    let started = Instant::now();
    reply(&app, thread, "t1", "PONG");
    completed(&app, thread, "t1", TurnStatus::Success);
    let result = waiter.await.unwrap().unwrap();
    assert!(started.elapsed() < PROMPT, "woken by the signal, not the {:?} tick: {:?}", wait::FALLBACK_TICK, started.elapsed());
    assert_eq!(result["state"], "done", "{result}");
    assert_eq!(result["final_text"], "PONG");
    assert_eq!(result["final_text_truncated"], false);
    assert_eq!(result["settled"], true);
    assert_eq!(result["timed_out"], false);
    assert_eq!(result["turn_id"], "t1");
    assert_eq!(result["failure"], Value::Null);
    assert_eq!(result["selected_turn"]["outcome"], "success");
    assert_eq!(result["pending"], json!({"approvals": [], "questions": [], "secrets": []}));
    assert_eq!(result["status"]["phase"], "completed");
    // final_message_id is the row thread_read returns for that text.
    let page = execute(app.handle(), &ControlCaller::trusted(), "thread_read", target(&workspace, thread, json!({}))).await.unwrap();
    let message = page["messages"].as_array().unwrap().iter().find(|m| m["message_id"] == result["final_message_id"]).expect("a thread_read row");
    assert_eq!(message["content"], "PONG");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_returns_needs_input_when_an_approval_opens() {
    let (app, provider, _root, workspace) = fixture().await;
    let thread = launched(&app, &workspace, "wait-approval-launch").await;
    running(&app, &thread, "t1");
    let trusted = ControlCaller::trusted();
    let waiter = spawn_wait(&app, &trusted, target(&workspace, &thread, json!({"timeout_ms": 5000})));
    parked().await;
    let started = Instant::now();
    forward_event(app.handle(), Event::RequestOpened {
        thread_id: ThreadId(thread.clone()), turn_id: TurnId("t1".into()), request_id: RequestId("approval-1".into()),
        request_kind: "tool_approval".into(), payload: json!({"private_tool_args": "synthetic confidential"}),
        tool_use_id: None, subagent_id: None,
    });
    let opened = waiter.await.unwrap().unwrap();
    assert!(started.elapsed() < PROMPT, "{:?}", started.elapsed());
    assert_eq!(opened["state"], "needs_input", "{opened}");
    assert_eq!(opened["settled"], false);
    assert_eq!(opened["timed_out"], false);
    let handle = opened["status"]["pending_approvals"][0]["request_id"].clone();
    assert_eq!(opened["pending"]["approvals"][0]["request_id"], handle);
    assert_eq!(opened["pending"]["approvals"][0]["turn_id"], "t1");
    assert_eq!(opened["pending"]["approvals"][0]["request_kind"], "tool_approval");
    assert_ne!(handle, "approval-1", "outside callers see the outside handle, never the provider's id");
    assert!(!opened.to_string().contains("private_tool_args"));
    // The same view again with since: it blocks instead of spinning.
    let cursor = opened["wait_cursor"].clone();
    let since = wait_for(&app, &trusted, target(&workspace, &thread, json!({"since": cursor, "timeout_ms": 100}))).await.unwrap();
    assert_eq!(since["timed_out"], true);
    assert_eq!(since["state"], "needs_input");
    assert_eq!(since["wait_cursor"], cursor);
    // Answering the approval is the next change.
    let waiter = spawn_wait(&app, &trusted, target(&workspace, &thread, json!({"since": cursor, "timeout_ms": 5000})));
    parked().await;
    let replied = execute(app.handle(), &trusted, "thread_respond", target(&workspace, &thread, json!({
        "client_request_id": "wait-deny", "request_id": handle, "decision": "deny",
    }))).await.unwrap();
    assert_eq!(settle_operation(&app, &replied).await["state"], "succeeded");
    forward_event(app.handle(), Event::RequestResolved {
        thread_id: ThreadId(thread.clone()), request_id: RequestId("approval-1".into()),
        decision: crate::agent_provider::ApprovalDecision::Deny { message: "Denied".into() },
    });
    let resumed = waiter.await.unwrap().unwrap();
    assert_eq!(resumed["state"], "running", "{resumed}");
    assert_eq!(resumed["timed_out"], false);
    assert_eq!(resumed["pending"]["approvals"], json!([]));
    assert!(provider.calls.snapshot().iter().any(|call| matches!(call, mock_agent_provider::MockCall::RespondToRequest(t, r) if t.0 == thread && r.0 == "approval-1")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_reports_an_open_question_after_the_turn_completed() {
    use crate::agent_provider::QuestionResolution;
    use crate::commands::async_questions::{agent_chat_answer_question, QuestionAction};
    let (app, _, _root, workspace) = fixture().await;
    let mut provider = mock_agent_provider::MockAgentProvider::new(ProviderKind::Claude);
    provider.async_questions = true;
    app.state::<crate::commands::agent_chat::ProviderRegistry>().set_claude(Arc::new(provider)).await;
    let thread = launched(&app, &workspace, "wait-question-launch").await;
    running(&app, &thread, "t1");
    reply(&app, &thread, "t1", "I need one decision before I continue.");
    forward_event(app.handle(), Event::QuestionsAsked {
        thread_id: ThreadId(thread.clone()),
        question: UserQuestionSet {
            id: "storage".into(), target: "provider-route".into(), source_item_id: "item-q".into(), source_turn_id: "t1".into(),
            text: "Pick the storage".into(),
            questions: vec![UserQuestion { title: "Which database?".into(), options: vec!["SQLite".into(), "PostgreSQL".into()] }],
            subagent_id: None,
        },
    });
    completed(&app, &thread, "t1", TurnStatus::Success);
    let trusted = ControlCaller::trusted();
    let asked = wait_for(&app, &trusted, target(&workspace, &thread, json!({"timeout_ms": 1000}))).await.unwrap();
    assert_eq!(asked["state"], "needs_input", "{asked}");
    assert_eq!(asked["settled"], true);
    assert_eq!(asked["timed_out"], false);
    let question = &asked["pending"]["questions"][0];
    assert_eq!(question["question_id"], "storage");
    assert_eq!(question["turn_id"], "t1");
    assert_eq!(question["text"], "Pick the storage");
    assert_eq!(question["questions"], json!([{"title": "Which database?", "options": ["SQLite", "PostgreSQL"]}]));
    assert!(question.get("target").is_none() && question.get("source_item_id").is_none(), "{question}");
    // An answer, as an outside thread_answer gives it, wakes a since-wait.
    let waiter = spawn_wait(&app, &trusted, target(&workspace, &thread, json!({"since": asked["wait_cursor"], "timeout_ms": 5000})));
    parked().await;
    let started = Instant::now();
    let resolution = agent_chat_answer_question(app.handle().clone(), ThreadId(thread.clone()), "storage".into(), QuestionAction::Answer {
        answers: vec!["SQLite".into()], submission_id: uuid::Uuid::new_v4().to_string(), retry_unknown: false, permission_mode_override: None,
    }).await.unwrap();
    assert!(matches!(resolution, QuestionResolution::Answered { .. }), "{resolution:?}");
    let answered = waiter.await.unwrap().unwrap();
    assert!(started.elapsed() < PROMPT, "{:?}", started.elapsed());
    assert_eq!(answered["state"], "done", "{answered}");
    assert_eq!(answered["final_text"], "I need one decision before I continue.");
    assert_eq!(answered["pending"]["questions"], json!([]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_reports_an_open_secret_card_without_its_value() {
    let (app, _provider, _root, workspace) = fixture().await;
    let thread = "wait-secret-card";
    persisted(&app, &workspace, thread);
    running(&app, thread, "t1");
    let trusted = ControlCaller::trusted();
    let waiter = spawn_wait(&app, &trusted, target(&workspace, thread, json!({"timeout_ms": 5000})));
    parked().await;
    let started = Instant::now();
    let request = crate::commands::secret_requests::open_for_test(thread, Some("t1"), "OpenAI API key");
    let asked = waiter.await.unwrap().unwrap();
    assert!(started.elapsed() < PROMPT, "{:?}", started.elapsed());
    assert_eq!(asked["state"], "needs_input", "{asked}");
    assert_eq!(asked["settled"], false);
    let card = &asked["pending"]["secrets"][0];
    assert_eq!(card["request_id"], request);
    assert_eq!(card["label"], "OpenAI API key");
    assert!(card["reason"].as_str().is_some_and(|reason| !reason.is_empty()));
    assert!(card["expires_at_ms"].as_i64().is_some_and(|at| at > 0));
    // Stopping the chat ends its cards; that wakes a since-wait too.
    let waiter = spawn_wait(&app, &trusted, target(&workspace, thread, json!({"since": asked["wait_cursor"], "timeout_ms": 5000})));
    parked().await;
    crate::commands::secret_requests::stop_thread(app.handle(), thread);
    let ended = waiter.await.unwrap().unwrap();
    assert_eq!(ended["state"], "running", "{ended}");
    assert_eq!(ended["pending"]["secrets"], json!([]));
    for result in [&asked, &ended] {
        let mut found = Vec::new();
        keys(result, &mut found);
        assert!(!found.iter().any(|key| key == "secret" || key == "value"), "{result}");
    }
}

#[tokio::test]
async fn wait_failed_turn_reports_a_safe_failure_code() {
    let (app, _provider, _root, workspace) = fixture().await;
    let private = "synthetic private diagnostic";
    for (index, (status, failure)) in [
        (TurnStatus::Error { subtype: "provider_failure".into(), message: private.into() }, "error"),
        (TurnStatus::Error { subtype: "interrupted".into(), message: private.into() }, "interrupted"),
        (TurnStatus::MaxTurns, "max_turns"),
        (TurnStatus::MaxBudget, "max_budget"),
    ].into_iter().enumerate() {
        let thread = format!("wait-failed-{index}");
        persisted(&app, &workspace, &thread);
        running(&app, &thread, "t1");
        reply(&app, &thread, "t1", "Partial work");
        completed(&app, &thread, "t1", status);
        let result = wait_for(&app, &ControlCaller::trusted(), target(&workspace, &thread, json!({"timeout_ms": 1000}))).await.unwrap();
        assert_eq!(result["state"], "failed", "{result}");
        assert_eq!(result["failure"], failure);
        assert_eq!(result["settled"], true);
        let status = wait::thread_status(app.handle(), target(&workspace, &thread, json!({}))).await.unwrap();
        assert_eq!(status["state"], "failed");
        assert_eq!(status["failure"], failure);
        assert!(!result.to_string().contains("private diagnostic") && !status.to_string().contains("private diagnostic"));
    }
}

#[tokio::test]
async fn wait_success_without_text_reads_failed_no_final_message() {
    let (app, _provider, _root, workspace) = fixture().await;
    let thread = "wait-silent";
    persisted(&app, &workspace, thread);
    running(&app, thread, "t1");
    reply(&app, thread, "t1", "   ");
    completed(&app, thread, "t1", TurnStatus::Success);
    let result = wait_for(&app, &ControlCaller::trusted(), target(&workspace, thread, json!({"timeout_ms": 1000}))).await.unwrap();
    assert_eq!(result["state"], "failed", "{result}");
    assert_eq!(result["failure"], "no_final_message");
    assert_eq!(result["final_text"], Value::Null);
    assert_eq!(result["final_message_id"], Value::Null);
    assert_eq!(result["settled"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_since_blocks_on_a_settled_thread_until_new_work() {
    let (app, _provider, _root, workspace) = fixture().await;
    let thread = "wait-since-settled";
    persisted(&app, &workspace, thread);
    running(&app, thread, "t1");
    reply(&app, thread, "t1", "First run done");
    completed(&app, thread, "t1", TurnStatus::Success);
    let trusted = ControlCaller::trusted();
    let done = wait_for(&app, &trusted, target(&workspace, thread, json!({"timeout_ms": 1000}))).await.unwrap();
    assert_eq!(done["state"], "done");
    let cursor = done["wait_cursor"].clone();
    // Without new work, a since-wait on a settled thread runs to its deadline.
    let started = Instant::now();
    let idle = wait_for(&app, &trusted, target(&workspace, thread, json!({"since": cursor, "timeout_ms": 200}))).await.unwrap();
    assert!(started.elapsed() >= Duration::from_millis(200));
    assert_eq!(idle["timed_out"], true);
    assert_eq!(idle["state"], "done");
    let waiter = spawn_wait(&app, &trusted, target(&workspace, thread, json!({"since": cursor, "timeout_ms": 2000})));
    tokio::time::sleep(Duration::from_millis(100)).await;
    let started = Instant::now();
    running(&app, thread, "t2");
    let next = waiter.await.unwrap().unwrap();
    assert!(started.elapsed() < PROMPT, "{:?}", started.elapsed());
    assert_eq!(next["state"], "running", "{next}");
    assert_eq!(next["turn_id"], "t2");
    assert_eq!(next["timed_out"], false);
    assert_ne!(next["wait_cursor"], cursor);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_after_send_never_returns_the_previous_run() {
    let (app, _provider, _root, workspace) = fixture().await;
    let trusted = ControlCaller::trusted();
    for follow in ["plain", "since"] {
        let thread = format!("wait-after-send-{follow}");
        persisted(&app, &workspace, &thread);
        running(&app, &thread, "t0");
        reply(&app, &thread, "t0", "OLD ANSWER");
        completed(&app, &thread, "t0", TurnStatus::Success);
        let done = wait_for(&app, &trusted, target(&workspace, &thread, json!({"timeout_ms": 1000}))).await.unwrap();
        assert_eq!(done["final_text"], "OLD ANSWER", "{done}");
        // thread_send returns once its delivery is admitted: the thread is
        // busy, but its current run is still the finished t0.
        let db = app.state::<DatabaseStore>();
        let delivery = format!("send-{follow}");
        db.admit_message_delivery(&delivery, &thread, &delivery, &json!({"origin": "controller"}), "epoch", 0, None, true).unwrap();
        let mut args = json!({"timeout_ms": 5000});
        if follow == "since" {
            // The send is the first change a since-loop sees.
            let busy = wait_for(&app, &trusted, target(&workspace, &thread, json!({"since": done["wait_cursor"], "timeout_ms": 1000}))).await.unwrap();
            assert_eq!(busy["state"], "running", "{busy}");
            assert_eq!(busy["timed_out"], false);
            assert_eq!(busy["final_text"], Value::Null);
            args["since"] = busy["wait_cursor"].clone();
        }
        let waiter = spawn_wait(&app, &trusted, target(&workspace, &thread, args));
        parked().await;
        db.finish_message_delivery(&delivery, "accepted", Some(&json!({"turn_id": "t1"})), None).unwrap();
        running(&app, &thread, "t1");
        let started = waiter.await.unwrap().unwrap();
        assert_eq!(started["state"], "running", "{follow}: the new run, never t0's answer: {started}");
        assert_eq!(started["turn_id"], "t1");
        assert_eq!(started["timed_out"], false);
        assert_eq!(started["final_text"], Value::Null);
        // The next wait follows t1 and reports t1's own result.
        let mut args = json!({"timeout_ms": 5000});
        if follow == "since" {
            args["since"] = started["wait_cursor"].clone();
        }
        let waiter = spawn_wait(&app, &trusted, target(&workspace, &thread, args));
        parked().await;
        reply(&app, &thread, "t1", "NEW ANSWER");
        completed(&app, &thread, "t1", TurnStatus::Success);
        let finished = waiter.await.unwrap().unwrap();
        assert_eq!(finished["state"], "done", "{follow}: {finished}");
        assert_eq!(finished["turn_id"], "t1");
        assert_eq!(finished["selected_turn"]["turn_id"], "t1");
        assert_eq!(finished["final_text"], "NEW ANSWER");
        assert_eq!(finished["timed_out"], false);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turn_wait_on_a_closed_session_reads_went_idle() {
    let (app, _provider, _root, workspace) = fixture().await;
    let thread = "wait-closed-mid-turn";
    persisted(&app, &workspace, thread);
    running(&app, thread, "t1");
    reply(&app, thread, "t1", "Halfway there");
    let trusted = ControlCaller::trusted();
    let args = || target(&workspace, thread, json!({"turn_id": "t1", "timeout_ms": 5000}));
    let waiter = spawn_wait(&app, &trusted, args());
    parked().await;
    let started = Instant::now();
    // Stopping a session mid-turn closes it; no TurnCompleted is ever sent
    // for the run it cut off.
    forward_event(app.handle(), Event::SessionStateChanged { thread_id: ThreadId(thread.into()), status: SessionStatus::Closed });
    let stopped = waiter.await.unwrap().unwrap();
    assert!(started.elapsed() < PROMPT, "{:?}", started.elapsed());
    assert_eq!(stopped["state"], "failed", "{stopped}");
    assert_eq!(stopped["failure"], "went_idle");
    assert_eq!(stopped["turn_id"], "t1");
    assert_eq!(stopped["timed_out"], false);
    assert_eq!(stopped["final_text"], "Halfway there");
    // The legacy flag still needs a recorded end.
    assert_eq!(stopped["settled"], false);
    assert_eq!(stopped["selected_turn"], Value::Null);
    // A wait that starts after the stop returns at once, and every view agrees.
    let started = Instant::now();
    let again = wait_for(&app, &trusted, args()).await.unwrap();
    assert!(started.elapsed() < PROMPT, "{:?}", started.elapsed());
    let plain = wait_for(&app, &trusted, target(&workspace, thread, json!({"timeout_ms": 5000}))).await.unwrap();
    let status = wait::thread_status(app.handle(), target(&workspace, thread, json!({}))).await.unwrap();
    for view in [&again, &plain, &status] {
        assert_eq!(view["state"], "failed", "{view}");
        assert_eq!(view["failure"], "went_idle");
    }
    assert_eq!(again["timed_out"], false);
    assert_eq!(again["wait_cursor"], stopped["wait_cursor"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turn_wait_on_a_run_stopped_before_a_newer_one_reads_went_idle() {
    let (app, _provider, _root, workspace) = fixture().await;
    let thread = "wait-superseded";
    persisted(&app, &workspace, thread);
    person_started(&app, thread, "t1");
    running(&app, thread, "t1");
    reply(&app, thread, "t1", "Halfway there");
    // The person presses Stop, which records no end for t1, and sends a new
    // message: the session starts again and runs t2.
    session(&app, thread, SessionStatus::Closed);
    session(&app, thread, SessionStatus::Starting);
    person_started(&app, thread, "t2");
    running(&app, thread, "t2");
    let trusted = ControlCaller::trusted();
    let started = Instant::now();
    let stopped = wait_for(&app, &trusted, target(&workspace, thread, json!({"turn_id": "t1", "timeout_ms": 2000}))).await.unwrap();
    assert!(started.elapsed() < PROMPT, "t1 is over, so the wait returns at once: {:?}", started.elapsed());
    assert_eq!(stopped["state"], "failed", "{stopped}");
    assert_eq!(stopped["failure"], "went_idle");
    assert_eq!(stopped["turn_id"], "t1");
    assert_eq!(stopped["timed_out"], false);
    assert_eq!(stopped["final_text"], "Halfway there");
    // The legacy flag still needs a recorded end.
    assert_eq!(stopped["settled"], false);
    assert_eq!(stopped["selected_turn"], Value::Null);
    // The newer run is unaffected.
    let current = wait_for(&app, &trusted, target(&workspace, thread, json!({"turn_id": "t2", "timeout_ms": 100}))).await.unwrap();
    assert_eq!((current["state"].as_str(), current["timed_out"].as_bool()), (Some("running"), Some(true)), "{current}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turn_wait_on_a_new_run_the_live_view_has_not_reached_keeps_waiting() {
    let (app, _provider, _root, workspace) = fixture().await;
    let thread = "wait-ahead-of-the-live-view";
    persisted(&app, &workspace, thread);
    person_started(&app, thread, "t1");
    running(&app, thread, "t1");
    session(&app, thread, SessionStatus::Closed);
    // A receipt can name t2 before the restarted session's events arrive:
    // the live view still shows the stopped t1, which is the older run.
    person_started(&app, thread, "t2");
    let trusted = ControlCaller::trusted();
    let ahead = wait_for(&app, &trusted, target(&workspace, thread, json!({"turn_id": "t2", "timeout_ms": 300}))).await.unwrap();
    assert_eq!((ahead["state"].as_str(), ahead["timed_out"].as_bool()), (Some("running"), Some(true)), "{ahead}");
    let stopped = wait_for(&app, &trusted, target(&workspace, thread, json!({"turn_id": "t1", "timeout_ms": 300}))).await.unwrap();
    assert_eq!(stopped["failure"], "went_idle", "{stopped}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turn_wait_on_a_turn_the_thread_never_ran_is_an_invalid_target() {
    let (app, _provider, _root, workspace) = fixture().await;
    let (thread, other) = ("wait-unknown-turn", "wait-unknown-turn-other");
    for (thread, turn) in [(thread, "t1"), (other, "theirs")] {
        persisted(&app, &workspace, thread);
        running(&app, thread, turn);
        reply(&app, thread, turn, "done");
        completed(&app, thread, turn, TurnStatus::Success);
    }
    let trusted = ControlCaller::trusted();
    // A typo, and a run of another thread.
    for turn in ["no-such-turn", "theirs"] {
        let started = Instant::now();
        let refused = wait_for(&app, &trusted, target(&workspace, thread, json!({"turn_id": turn, "timeout_ms": 2000}))).await.unwrap_err();
        assert_eq!(refused.code, ErrorCode::InvalidTarget, "{turn}: {refused}");
        assert!(started.elapsed() < PROMPT, "{turn}: {:?}", started.elapsed());
    }
    // While the thread is busy the turn may be a run whose first row has yet
    // to land, so the wait goes on, and ends once the thread is idle without it.
    running(&app, thread, "t2");
    let waiter = spawn_wait(&app, &trusted, target(&workspace, thread, json!({"turn_id": "no-such-turn", "timeout_ms": 5000})));
    parked().await;
    assert!(!waiter.is_finished());
    let started = Instant::now();
    reply(&app, thread, "t2", "done too");
    completed(&app, thread, "t2", TurnStatus::Success);
    let refused = waiter.await.unwrap().unwrap_err();
    assert_eq!(refused.code, ErrorCode::InvalidTarget, "{refused}");
    assert!(started.elapsed() < PROMPT, "{:?}", started.elapsed());
}

/// A persisted thread Codemux has not reopened, as after a restart, reads
/// `unknown` and never settles. With no live session and nothing under way,
/// no run can still bring a turn it has no record of, so the wait refuses
/// that turn at once instead of reading `running` until every deadline. A
/// message Codemux is still delivering may start the run, so the wait goes on.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turn_wait_on_an_unknown_turn_of_a_thread_at_rest_is_an_invalid_target() {
    let (app, _provider, _root, workspace) = fixture().await;
    // A thread that never ran, and one whose run was cut off by an exit.
    let (empty, cut_off) = ("wait-unknown-turn-at-rest", "wait-unknown-turn-cut-off");
    persisted(&app, &workspace, empty);
    persisted(&app, &workspace, cut_off);
    person_started(&app, cut_off, "t1");
    let trusted = ControlCaller::trusted();
    let typo = |thread: &str, timeout_ms: u64| target(&workspace, thread, json!({"turn_id": "no-such-turn", "timeout_ms": timeout_ms}));
    for thread in [empty, cut_off] {
        let status = wait::thread_status(app.handle(), target(&workspace, thread, json!({}))).await.unwrap();
        assert_eq!((status["phase"].as_str(), status["settled"].as_bool()), (Some("unknown"), Some(false)), "{status}");
        let started = Instant::now();
        let refused = wait_for(&app, &trusted, typo(thread, 2000)).await.unwrap_err();
        assert_eq!(refused.code, ErrorCode::InvalidTarget, "{thread}: {refused}");
        assert!(started.elapsed() < PROMPT, "{thread}: {:?}", started.elapsed());
    }
    let epoch = app.state::<NativeControlState>().epoch.clone();
    let payload = json!({"provider": "claude", "origin": "controller", "input": {"thread_id": empty, "text": "Next task"}});
    app.state::<DatabaseStore>()
        .admit_message_delivery("wait-unknown-turn-delivery", empty, "wait-unknown-turn-delivery", &payload, &epoch, 0, None, true)
        .unwrap();
    let delivering = wait_for(&app, &trusted, typo(empty, 300)).await.unwrap();
    assert_eq!((delivering["state"].as_str(), delivering["timed_out"].as_bool()), (Some("running"), Some(true)), "{delivering}");
}

/// A message queued behind the running t1, as `thread_send` takes it: its
/// receipt names the queue entry and no run yet.
async fn queued_send(app: &App, provider: &mock_agent_provider::MockAgentProvider, workspace: &str, thread: &str, key: &str) -> Value {
    provider.queue_next_send();
    let receipt = execute(app.handle(), &ControlCaller::trusted(), "thread_send", target(workspace, thread, json!({
        "client_request_id": key, "message": "Then do this", "delivery": "queue",
    }))).await.unwrap();
    let sent = settle_operation(app, &receipt).await;
    assert_eq!(sent["result"]["accepted_as"], "queued", "{sent}");
    sent
}

/// The queue entry a settled launch or send receipt names.
fn queued_id(receipt: &Value) -> String {
    receipt["result"]["turn"]["queued_id"].as_str().expect("the receipt names a queue entry").to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operation_wait_follows_a_queued_message_to_its_own_run() {
    let (app, provider, _root, workspace) = fixture().await;
    let thread = launched(&app, &workspace, "operation-wait-launch").await;
    person_started(&app, &thread, "t1");
    running(&app, &thread, "t1");
    let sent = queued_send(&app, &provider, &workspace, &thread, "operation-wait-queued").await;
    let (operation, entry) = (sent["operation_id"].as_str().unwrap().to_string(), queued_id(&sent));
    let trusted = ControlCaller::trusted();
    let args = |extra: Value| {
        let mut args = target(&workspace, &thread, json!({"operation_id": operation}));
        args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        args
    };
    // Still queued: running, with no run of its own yet.
    let queued = wait_for(&app, &trusted, args(json!({"timeout_ms": 200}))).await.unwrap();
    assert_eq!((queued["state"].as_str(), queued["timed_out"].as_bool()), (Some("running"), Some(true)), "{queued}");
    assert_eq!((&queued["turn_id"], &queued["final_text"]), (&Value::Null, &Value::Null));
    assert_eq!(queued["operation"], json!({"operation_id": operation, "state": "succeeded", "error": null}));
    // The run ahead ends; its answer is never this message's.
    let waiter = spawn_wait(&app, &trusted, args(json!({"timeout_ms": 5000})));
    parked().await;
    reply(&app, &thread, "t1", "EARLIER ANSWER");
    completed(&app, &thread, "t1", TurnStatus::Success);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!waiter.is_finished(), "the run ahead ending is not this message's change");
    // The provider dispatches the entry as t2, as forward_event records it:
    // the receipt names the run, then the live view moves to it.
    let db = app.state::<DatabaseStore>();
    db.finish_message_delivery(&operation, "accepted", Some(&json!({"turn_id": "t2", "queued_id": null, "steered": false})), None).unwrap();
    person_started(&app, &thread, "t2");
    app.state::<NativeControlState>().observe_event(&Event::QueuedTurnDispatched {
        thread_id: ThreadId(thread.clone()), queued_id: entry, turn_id: TurnId("t2".into()),
        text: "Then do this".into(), steered: false,
    });
    let started = waiter.await.unwrap().unwrap();
    assert_eq!(started["state"], "running", "{started}");
    assert_eq!((started["turn_id"].as_str(), started["timed_out"].as_bool()), (Some("t2"), Some(false)));
    assert_eq!(started["final_text"], Value::Null);
    // The next wait follows t2 to its end.
    let waiter = spawn_wait(&app, &trusted, args(json!({"since": started["wait_cursor"], "timeout_ms": 5000})));
    parked().await;
    reply(&app, &thread, "t2", "MINE");
    completed(&app, &thread, "t2", TurnStatus::Success);
    let done = waiter.await.unwrap().unwrap();
    assert_eq!(done["state"], "done", "{done}");
    assert_eq!((done["turn_id"].as_str(), done["final_text"].as_str()), (Some("t2"), Some("MINE")));
    // Once it ended, a plain wait returns at once with the same answer.
    let again = wait_for(&app, &trusted, args(json!({"timeout_ms": 5000}))).await.unwrap();
    assert_eq!((again["state"].as_str(), again["final_text"].as_str(), again["timed_out"].as_bool()), (Some("done"), Some("MINE"), Some(false)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operation_wait_reports_a_cancelled_message_with_its_receipt_error() {
    let (app, provider, _root, workspace) = fixture().await;
    let thread = launched(&app, &workspace, "operation-cancel-launch").await;
    person_started(&app, &thread, "t1");
    running(&app, &thread, "t1");
    let sent = queued_send(&app, &provider, &workspace, &thread, "operation-cancel-queued").await;
    let (operation, queued) = (sent["operation_id"].as_str().unwrap().to_string(), queued_id(&sent));
    let trusted = ControlCaller::trusted();
    let args = target(&workspace, &thread, json!({"operation_id": operation, "timeout_ms": 5000}));
    let waiter = spawn_wait(&app, &trusted, args.clone());
    parked().await;
    // The person removes the entry from the queue.
    app.state::<DatabaseStore>().finish_message_delivery(&operation, "cancelled", None, Some("Queued message cancelled")).unwrap();
    app.state::<NativeControlState>().observe_event(&Event::QueuedTurnCancelled {
        thread_id: ThreadId(thread.clone()), queued_id: queued,
    });
    let cancelled = waiter.await.unwrap().unwrap();
    assert_eq!(cancelled["state"], "failed", "{cancelled}");
    assert_eq!(cancelled["failure"], "operation_failed");
    assert_eq!(cancelled["timed_out"], false);
    assert_eq!((&cancelled["turn_id"], &cancelled["final_text"]), (&Value::Null, &Value::Null));
    assert_eq!(cancelled["operation"]["state"], "failed");
    assert_eq!(cancelled["operation"]["error"]["code"], "cancelled");
    assert_eq!(cancelled["status"]["thread_id"], thread.as_str());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operation_wait_follows_a_launch_from_its_first_receipt() {
    let (app, provider, _root, workspace) = fixture().await;
    let (entered, release) = provider.hold_next_start();
    let trusted = ControlCaller::trusted();
    let receipt = execute(app.handle(), &trusted, "thread_launch", json!({
        "workspace_id": workspace, "client_request_id": "operation-launch", "provider": "claude", "permission_mode": "default",
        "message": "Say PONG",
    })).await.unwrap();
    entered.notified().await;
    let thread = receipt["thread_id"].as_str().unwrap().to_string();
    let args = |extra: Value| {
        let mut args = target(&workspace, &thread, json!({"operation_id": receipt["operation_id"]}));
        args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        args
    };
    // Straight from the receipt, while the launch is still starting the provider.
    let starting = wait_for(&app, &trusted, args(json!({"timeout_ms": 200}))).await.unwrap();
    assert_eq!((starting["state"].as_str(), starting["timed_out"].as_bool()), (Some("running"), Some(true)), "{starting}");
    assert_eq!(starting["turn_id"], Value::Null);
    assert_eq!(starting["operation"]["state"], "running");
    let waiter = spawn_wait(&app, &trusted, args(json!({"timeout_ms": 5000})));
    parked().await;
    let started = Instant::now();
    release.notify_one();
    let running = waiter.await.unwrap().unwrap();
    assert!(started.elapsed() < PROMPT, "the settled launch wakes the wait: {:?}", started.elapsed());
    assert_eq!((running["state"].as_str(), running["turn_id"].as_str()), (Some("running"), Some("mock-turn")), "{running}");
    reply(&app, &thread, "mock-turn", "PONG");
    completed(&app, &thread, "mock-turn", TurnStatus::Success);
    let done = wait_for(&app, &trusted, args(json!({"since": running["wait_cursor"], "timeout_ms": 5000}))).await.unwrap();
    assert_eq!((done["state"].as_str(), done["final_text"].as_str()), (Some("done"), Some("PONG")), "{done}");
}

/// A launch whose first message the provider queued, as Hermes always does
/// and any provider does while its session is busy. Its receipt settles once,
/// naming only the queue entry, and is never rewritten afterwards.
async fn queued_launch(app: &App, provider: &mock_agent_provider::MockAgentProvider, workspace: &str, key: &str) -> (String, String, String) {
    provider.queue_next_send();
    let receipt = execute(app.handle(), &ControlCaller::trusted(), "thread_launch", json!({
        "workspace_id": workspace, "client_request_id": key, "provider": "claude", "permission_mode": "default",
        "message": "Say PONG",
    })).await.unwrap();
    let launched = settle_operation(app, &receipt).await;
    assert_eq!((launched["state"].as_str(), launched["result"]["turn"]["turn_id"].as_str()), (Some("succeeded"), Some("")), "{launched}");
    let thread = launched["thread_id"].as_str().unwrap().to_string();
    (thread, launched["operation_id"].as_str().unwrap().to_string(), queued_id(&launched))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operation_wait_follows_a_launchs_queued_message_to_its_run() {
    let (app, provider, _root, workspace) = fixture().await;
    let (thread, operation, queued) = queued_launch(&app, &provider, &workspace, "operation-queued-launch").await;
    let trusted = ControlCaller::trusted();
    let args = |extra: Value| {
        let mut args = target(&workspace, &thread, json!({"operation_id": operation}));
        args.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        args
    };
    let waiting = wait_for(&app, &trusted, args(json!({"timeout_ms": 200}))).await.unwrap();
    assert_eq!((waiting["state"].as_str(), waiting["timed_out"].as_bool()), (Some("running"), Some(true)), "{waiting}");
    assert_eq!(waiting["turn_id"], Value::Null);
    let waiter = spawn_wait(&app, &trusted, args(json!({"timeout_ms": 5000})));
    parked().await;
    let started = Instant::now();
    // The provider dispatches the entry. The event bridge records the run on
    // the message's delivery; the launch receipt keeps naming only the entry.
    forward_event(app.handle(), Event::QueuedTurnDispatched {
        thread_id: ThreadId(thread.clone()), queued_id: queued.clone(), turn_id: TurnId("t1".into()),
        text: "Say PONG".into(), steered: false,
    });
    let running = waiter.await.unwrap().unwrap();
    assert!(started.elapsed() < PROMPT, "the dispatch wakes the wait: {:?}", started.elapsed());
    assert_eq!((running["state"].as_str(), running["turn_id"].as_str(), running["timed_out"].as_bool()), (Some("running"), Some("t1"), Some(false)), "{running}");
    let waiter = spawn_wait(&app, &trusted, args(json!({"since": running["wait_cursor"], "timeout_ms": 5000})));
    parked().await;
    reply(&app, &thread, "t1", "PONG");
    completed(&app, &thread, "t1", TurnStatus::Success);
    let done = waiter.await.unwrap().unwrap();
    assert_eq!((done["state"].as_str(), done["turn_id"].as_str(), done["final_text"].as_str()), (Some("done"), Some("t1"), Some("PONG")), "{done}");
    assert_eq!(done["operation"], json!({"operation_id": operation, "state": "succeeded", "error": null}));
    let receipt = execute(app.handle(), &trusted, "operation_status", json!({"operation_id": operation})).await.unwrap();
    assert_eq!(queued_id(&receipt), queued, "the launch receipt still names only the entry: {receipt}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operation_wait_ends_when_a_launchs_queued_message_never_runs() {
    let (app, provider, _root, workspace) = fixture().await;
    let trusted = ControlCaller::trusted();
    // The person removes the entry from the queue.
    let (thread, operation, queued) = queued_launch(&app, &provider, &workspace, "operation-queued-launch-cancel").await;
    let waiter = spawn_wait(&app, &trusted, target(&workspace, &thread, json!({"operation_id": operation, "timeout_ms": 5000})));
    parked().await;
    let started = Instant::now();
    forward_event(app.handle(), Event::QueuedTurnCancelled { thread_id: ThreadId(thread.clone()), queued_id: queued });
    let cancelled = waiter.await.unwrap().unwrap();
    assert!(started.elapsed() < PROMPT, "{:?}", started.elapsed());
    assert_eq!((cancelled["state"].as_str(), cancelled["failure"].as_str()), (Some("failed"), Some("operation_failed")), "{cancelled}");
    assert_eq!((&cancelled["turn_id"], &cancelled["final_text"], &cancelled["timed_out"]), (&Value::Null, &Value::Null, &json!(false)));
    // The launch itself succeeded; its message did not run.
    assert_eq!(cancelled["operation"]["state"], "succeeded");
    // A stop holds a queued message for the person instead.
    let (thread, operation, _) = queued_launch(&app, &provider, &workspace, "operation-queued-launch-stop").await;
    app.state::<DatabaseStore>().stop_message_deliveries(&thread).unwrap();
    let started = Instant::now();
    let held = wait_for(&app, &trusted, target(&workspace, &thread, json!({"operation_id": operation, "timeout_ms": 2000}))).await.unwrap();
    assert!(started.elapsed() < PROMPT, "{:?}", started.elapsed());
    assert_eq!((held["state"].as_str(), held["failure"].as_str()), (Some("failed"), Some("delivery_needs_attention")), "{held}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operation_wait_refuses_what_it_cannot_follow() {
    let (app, _provider, _root, workspace) = fixture().await;
    let trusted = ControlCaller::trusted();
    let receipt = execute(app.handle(), &trusted, "thread_launch", json!({
        "workspace_id": workspace, "client_request_id": "operation-refusals", "provider": "claude", "permission_mode": "default",
    })).await.unwrap();
    let launch = settle_operation(&app, &receipt).await;
    assert_eq!(launch["state"], "succeeded", "{launch}");
    let (thread, operation) = (launch["thread_id"].as_str().unwrap().to_string(), launch["operation_id"].clone());
    let other = launched(&app, &workspace, "operation-refusals-other").await;
    let code = |result: Result<Value, ControlError>| result.unwrap_err().code;
    // A launch without a message started no run.
    assert_eq!(code(wait_for(&app, &trusted, target(&workspace, &thread, json!({"operation_id": operation, "timeout_ms": 1000}))).await), ErrorCode::InvalidTarget);
    // It is not the other thread's operation.
    assert_eq!(code(wait_for(&app, &trusted, target(&workspace, &other, json!({"operation_id": operation, "timeout_ms": 1000}))).await), ErrorCode::InvalidTarget);
    // Another client never sees it, and nobody sees one that does not exist.
    let named = ControlCaller::local(Some(&super::ClientName::parse("Other Client").unwrap()), ControlAccess::FullAccess);
    assert_eq!(code(wait_for(&app, &named, target(&workspace, &thread, json!({"operation_id": operation, "timeout_ms": 1000}))).await), ErrorCode::OperationNotFound);
    assert_eq!(code(wait_for(&app, &trusted, target(&workspace, &thread, json!({"operation_id": "no-such-operation", "timeout_ms": 1000}))).await), ErrorCode::OperationNotFound);
    // A receipt that starts no run, and both selectors at once.
    let interrupt = execute(app.handle(), &trusted, "thread_interrupt", target(&workspace, &thread, json!({"client_request_id": "operation-refusals-interrupt"}))).await.unwrap();
    assert_eq!(code(wait_for(&app, &trusted, target(&workspace, &thread, json!({"operation_id": interrupt["operation_id"], "timeout_ms": 1000}))).await), ErrorCode::InvalidTarget);
    assert_eq!(code(wait_for(&app, &trusted, target(&workspace, &thread, json!({"operation_id": operation, "turn_id": "t1"}))).await), ErrorCode::InvalidArguments);
    assert_eq!(code(wait_for(&app, &trusted, target(&workspace, &thread, json!({"operation_id": " padded "}))).await), ErrorCode::InvalidArguments);
}

#[tokio::test]
async fn wait_limits_depend_on_the_caller() {
    let (app, _provider, _root, workspace, _own, _authority, actor) = native_caller_fixture("bypassPermissions").await;
    let agent = super::native::qualify(app.handle(), &actor).await.unwrap();
    let trusted = ControlCaller::trusted();
    let idle = "limits-idle";
    persisted(&app, &workspace, idle);
    running(&app, idle, "t1");
    reply(&app, idle, "t1", "done");
    completed(&app, idle, "t1", TurnStatus::Success);
    let at = |timeout: u64| target(&workspace, idle, json!({"timeout_ms": timeout}));
    let code = |result: Result<Value, ControlError>| result.unwrap_err().code;
    assert_eq!(wait_for(&app, &trusted, at(600_000)).await.unwrap()["state"], "done");
    assert_eq!(code(wait_for(&app, &trusted, at(600_001)).await), ErrorCode::InvalidArguments);
    assert_eq!(code(wait_for(&app, &trusted, at(0)).await), ErrorCode::InvalidArguments);
    assert_eq!(wait_for(&app, &agent, at(50_000)).await.unwrap()["state"], "done");
    let refused = wait_for(&app, &agent, at(50_001)).await.unwrap_err();
    assert_eq!(refused.code, ErrorCode::InvalidArguments);
    assert!(refused.message.contains("50000"), "{refused}");
    for (field, value) in [("since", json!("")), ("since", json!("x".repeat(65))), ("turn_id", json!(" padded "))] {
        let args = target(&workspace, idle, json!({field: value}));
        assert_eq!(code(wait_for(&app, &trusted, args).await), ErrorCode::InvalidArguments, "{field}");
    }
    assert_eq!(code(wait_for(&app, &trusted, target(&workspace, idle, json!({"poll": true}))).await), ErrorCode::InvalidArguments);
    // Omitted timeouts default to 50 s, outside and for a chat alike.
    let busy = "limits-busy";
    persisted(&app, &workspace, busy);
    running(&app, busy, "t1");
    tokio::time::pause();
    for (caller, expected) in [(&trusted, wait::DEFAULT_WAIT_MS), (&agent, wait::MAX_AGENT_WAIT_MS)] {
        let started = Instant::now();
        let result = wait_for(&app, caller, target(&workspace, busy, json!({}))).await.unwrap();
        let elapsed = started.elapsed();
        assert_eq!(result["timed_out"], true, "{result}");
        assert_eq!(result["state"], "running");
        let expected = Duration::from_millis(expected);
        assert!(elapsed >= expected && elapsed < expected + Duration::from_secs(1), "{elapsed:?} vs {expected:?}");
    }
    tokio::time::resume();
}

#[tokio::test]
async fn a_chat_cannot_wait_for_itself() {
    let (app, _provider, _root, workspace, own, _authority, actor) = native_caller_fixture("bypassPermissions").await;
    let agent = super::native::qualify(app.handle(), &actor).await.unwrap();
    // Its own run is in progress, as it is whenever the chat calls a tool.
    app.state::<NativeControlState>().observe_event(&Event::SessionStateChanged {
        thread_id: ThreadId(own.clone()), status: SessionStatus::Running { active_turn: TurnId("own-turn".into()) },
    });
    let refused = wait_for(&app, &agent, target(&workspace, &own, json!({"timeout_ms": 1000}))).await.unwrap_err();
    assert_eq!(refused.code, ErrorCode::InvalidTarget, "{refused}");
    let routed = super::native::call(app.handle().clone(), actor, "thread_wait".into(), json!({"thread_id": own, "timeout_ms": 1000})).await.unwrap_err();
    assert!(routed.starts_with("invalid_target"), "{routed}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_ends_promptly_when_the_calling_chat_is_stopped() {
    let (app, _provider, _root, workspace, _own, authority, actor) = native_caller_fixture("bypassPermissions").await;
    let agent = super::native::qualify(app.handle(), &actor).await.unwrap();
    let busy = "stopped-caller-target";
    persisted(&app, &workspace, busy);
    running(&app, busy, "t1");
    let waiter = spawn_wait(&app, &agent, target(&workspace, busy, json!({"timeout_ms": 5000})));
    parked().await;
    let started = Instant::now();
    authority.revoke();
    let ended = waiter.await.unwrap().unwrap_err();
    assert!(started.elapsed() < PROMPT, "{:?}", started.elapsed());
    assert_eq!(ended.code, ErrorCode::StaleCaller, "{ended}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_ends_promptly_when_an_outside_grant_is_revoked() {
    let (app, _provider, _root, workspace) = fixture().await;
    let busy = "revoked-grant-target";
    persisted(&app, &workspace, busy);
    running(&app, busy, "t1");
    let caller = connector_caller(&app, "wait-client", "wait-grant", ControlAccess::FullAccess).await;
    let waiter = spawn_wait(&app, &caller, target(&workspace, busy, json!({"timeout_ms": 5000})));
    parked().await;
    let started = Instant::now();
    // The revoke itself wakes the wait (the connector's authority hook).
    crate::mcp_connector::agent_connector_revoke(app.handle().clone(), "wait-client".into()).unwrap();
    let ended = waiter.await.unwrap().unwrap_err();
    assert!(started.elapsed() < PROMPT, "{:?}", started.elapsed());
    assert_eq!(ended.code, ErrorCode::GrantRevoked, "{ended}");
    connector_off(&app);
}

#[tokio::test]
async fn wait_under_an_armed_usage_resume_reads_running() {
    let (app, _provider, _root, workspace) = fixture().await;
    let thread = "wait-usage-paused";
    persisted(&app, &workspace, thread);
    let state = app.state::<NativeControlState>();
    state.observe_event(&Event::SessionStateChanged {
        thread_id: ThreadId(thread.into()), status: SessionStatus::Running { active_turn: TurnId("t1".into()) },
    });
    let limited = Event::TurnCompleted {
        thread_id: ThreadId(thread.into()), turn_id: TurnId("t1".into()), usage: None,
        status: TurnStatus::Error { subtype: crate::agent_provider::events::RATE_LIMIT_SUBTYPE.into(), message: "rate limited".into() },
    };
    let db = app.state::<DatabaseStore>();
    db.append_agent_chat_message(thread, &serde_json::to_string(&limited).unwrap()).unwrap();
    state.observe_event(&limited);
    let resume_at = chrono::Utc::now().timestamp_millis() + 3_600_000;
    db.upsert_agent_chat_usage_resume(thread, "claude", Some(resume_at)).unwrap();
    let args = || target(&workspace, thread, json!({"timeout_ms": 1000}));
    let paused = wait_for(&app, &ControlCaller::trusted(), args()).await.unwrap();
    assert_eq!(paused["state"], "running", "an armed resume continues the work: {paused}");
    assert_eq!(paused["paused_until_ms"], resume_at);
    assert_eq!(paused["failure"], Value::Null);
    let status = wait::thread_status(app.handle(), target(&workspace, thread, json!({}))).await.unwrap();
    assert_eq!(status["state"], "running");
    assert_eq!(status["paused_until_ms"], resume_at);
    db.upsert_agent_chat_usage_resume(thread, "claude", None).unwrap();
    let stopped = wait_for(&app, &ControlCaller::trusted(), args()).await.unwrap();
    assert_eq!(stopped["state"], "failed", "{stopped}");
    assert_eq!(stopped["failure"], "usage_limit");
    assert_eq!(stopped["paused_until_ms"], Value::Null);
}

#[tokio::test]
async fn wait_delivery_needing_attention_reads_failed() {
    let (app, _provider, _root, workspace) = fixture().await;
    let thread = "wait-delivery-attention";
    persisted(&app, &workspace, thread);
    let db = app.state::<DatabaseStore>();
    db.admit_message_delivery("held", thread, "held", &json!({"origin": "controller"}), "epoch", 0, None, true).unwrap();
    db.refuse_message_delivery("held", "permission_ceiling: refused before send").unwrap();
    let result = wait_for(&app, &ControlCaller::trusted(), target(&workspace, thread, json!({"timeout_ms": 1000}))).await.unwrap();
    assert_eq!(result["status"]["phase"], "needs_attention", "{result}");
    assert_eq!(result["settled"], true);
    assert_eq!(result["state"], "failed");
    assert_eq!(result["failure"], "delivery_needs_attention");
}

#[tokio::test]
async fn wait_final_text_is_bounded() {
    let (app, _provider, _root, workspace) = fixture().await;
    let thread = "wait-long-reply";
    persisted(&app, &workspace, thread);
    running(&app, thread, "t1");
    // Two bytes per character: the bound counts characters.
    let long = "ü".repeat(20_000);
    reply(&app, thread, "t1", &long);
    completed(&app, thread, "t1", TurnStatus::Success);
    let result = wait_for(&app, &ControlCaller::trusted(), target(&workspace, thread, json!({"timeout_ms": 1000}))).await.unwrap();
    assert_eq!(result["state"], "done");
    assert_eq!(result["final_text"].as_str().unwrap().chars().count(), 16_000);
    assert_eq!(result["final_text_truncated"], true);
    // The whole message stays one thread_read away.
    let id = result["final_message_id"].as_i64().unwrap();
    let page = execute(app.handle(), &ControlCaller::trusted(), "thread_read", target(&workspace, thread, json!({"cursor": id - 1, "limit": 1}))).await.unwrap();
    assert_eq!(page["messages"][0]["message_id"], id);
    assert_eq!(page["messages"][0]["content"], long);
}

#[tokio::test]
async fn thread_status_reports_state_and_pending_additively() {
    let (app, _provider, _root, workspace) = fixture().await;
    let thread = "status-additive";
    persisted(&app, &workspace, thread);
    app.state::<NativeControlState>().observe_event(&Event::RequestOpened {
        thread_id: ThreadId(thread.into()), turn_id: TurnId("t1".into()), request_id: RequestId("provider-request".into()),
        request_kind: "tool".into(), payload: json!({}), tool_use_id: None, subagent_id: None,
    });
    let args = target(&workspace, thread, json!({}));
    let legacy = super::service::thread_status(app.handle(), args.clone()).await.unwrap();
    let status = wait::thread_status(app.handle(), args.clone()).await.unwrap();
    for (key, value) in legacy.as_object().unwrap() {
        assert_eq!(&status[key], value, "legacy key {key} must be unchanged");
    }
    assert_eq!(status["state"], "needs_input", "{status}");
    assert_eq!(status["failure"], Value::Null);
    assert_eq!(status["pending"]["approvals"][0]["request_id"], legacy["pending_approvals"][0]["request_id"]);
    assert_eq!(status["pending"]["questions"], json!([]));
    assert_eq!(status["pending"]["secrets"], json!([]));
    assert_eq!(status["paused_until_ms"], Value::Null);
    assert_eq!(status["wait_cursor"].as_str().unwrap().len(), 16);
    let mut extra = status.as_object().unwrap().keys().filter(|key| legacy.get(key.as_str()).is_none()).cloned().collect::<Vec<_>>();
    extra.sort();
    assert_eq!(extra, ["failure", "paused_until_ms", "pending", "state", "wait_cursor"]);
    // The routed tool is this function, and nothing changed in between.
    assert_eq!(execute(app.handle(), &ControlCaller::trusted(), "thread_status", args.clone()).await.unwrap(), status);
    let mut unknown = args;
    unknown["turn_id"] = json!("t1");
    assert_eq!(wait::thread_status(app.handle(), unknown).await.unwrap_err().code, ErrorCode::InvalidArguments);
}

#[tokio::test]
async fn missed_signal_is_covered_by_the_fallback_tick() {
    let (app, _provider, _root, workspace) = fixture().await;
    let thread = "wait-missed-signal";
    persisted(&app, &workspace, thread);
    // t1 has started, so the thread has a record of it.
    person_started(&app, thread, "t1");
    tokio::time::pause();
    let started = Instant::now();
    let waiter = spawn_wait(&app, &ControlCaller::trusted(), target(&workspace, thread, json!({"turn_id": "t1", "timeout_ms": 30_000})));
    parked().await;
    // Written straight to the database: nothing raises the signal.
    app.state::<DatabaseStore>().append_agent_chat_message(thread, &json!({
        "type": "turn_completed", "thread_id": thread, "turn_id": "t1", "status": {"kind": "success"}, "usage": null,
    }).to_string()).unwrap();
    let result = waiter.await.unwrap().unwrap();
    let elapsed = started.elapsed();
    assert_eq!(result["settled"], true, "{result}");
    assert_eq!(result["timed_out"], false);
    assert_eq!(result["selected_turn"]["outcome"], "success");
    assert!(elapsed <= wait::FALLBACK_TICK + Duration::from_millis(200), "{elapsed:?}");
    tokio::time::resume();
}
