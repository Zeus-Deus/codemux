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
