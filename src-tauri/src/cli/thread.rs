//! `codemux thread …`: launch agent threads, send them messages, wait for
//! their runs, read them and answer their approvals and questions.
//!
//! Mutations settle their receipt before they return. `--wait` then follows
//! the run with `thread_wait` slices until it is done, failed or needs
//! input. A wait never cancels work: a deadline only stops the CLI.

use super::args::{self, OutputArgs, RunArgs};
use super::door::{self, code, Door};
use super::output::{self, CliError, ErrorBody, Exit};
use super::verbs;
use clap::{Args, Subcommand, ValueEnum};
use serde::Serialize;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::time::Instant;

#[derive(Subcommand)]
pub enum ThreadCommand {
    /// Launch an agent thread, optionally in a new worktree, and print its id.
    Start(StartArgs),
    /// Send a follow-up message. Auto delivery starts, steers or queues;
    /// nothing is interrupted.
    Send(SendArgs),
    /// Wait for a thread's run to finish and print its reply.
    Wait(WaitArgs),
    /// Print a thread's messages, oldest first.
    Read(ReadArgs),
    /// List a workspace's threads.
    List(ListArgs),
    /// Show a thread's state and what it waits on. Exits 0 whatever the state.
    Status(StatusArgs),
    /// Answer a pending approval.
    Respond(RespondArgs),
    /// Answer a worker's pending question and print the turn that continues.
    Answer(AnswerArgs),
    /// Interrupt the running turn. The thread, its history and files stay.
    Interrupt(InterruptArgs),
    /// Stop the agent session and its queued work. Nothing is deleted.
    Stop(StopArgs),
}

#[derive(Args)]
#[command(group(clap::ArgGroup::new("permission").required(true).args(["permission_mode", "no_permission_mode"])))]
pub struct StartArgs {
    /// Workspace to launch in (default $CODEMUX_WORKSPACE_ID).
    #[arg(long, value_name = "ID")]
    pub workspace: Option<String>,
    /// claude, codex, cursor, grok, hermes, opencode or acp.
    #[arg(long, value_name = "PROVIDER")]
    pub provider: String,
    /// A configured provider instance (an account, Hermes profile or custom
    /// ACP agent), as agent_capabilities lists it.
    #[arg(long, value_name = "ID")]
    pub instance: Option<String>,
    /// A model id the provider advertises.
    #[arg(long, value_name = "MODEL")]
    pub model: Option<String>,
    /// A reasoning effort the model advertises.
    #[arg(long, value_name = "EFFORT")]
    pub effort: Option<String>,
    /// A permission mode the provider advertises.
    #[arg(long, value_name = "MODE")]
    pub permission_mode: Option<String>,
    /// Send no permission mode, for providers that have none.
    #[arg(long)]
    pub no_permission_mode: bool,
    /// Title for the thread.
    #[arg(long, value_name = "TITLE")]
    pub title: Option<String>,
    /// First message, or - to read it from stdin.
    #[arg(long, short, value_name = "MSG")]
    pub message: String,
    /// Create a worktree on this new branch and launch the thread there.
    #[arg(long, value_name = "BRANCH")]
    pub worktree_branch: Option<String>,
    /// Where the new branch starts (default: the repository's default branch).
    #[arg(long, value_name = "REF", requires = "worktree_branch")]
    pub worktree_base: Option<String>,
    #[command(flatten)]
    pub run: RunArgs,
    #[command(flatten)]
    pub out: OutputArgs,
}

#[derive(Args)]
pub struct SendArgs {
    pub thread: String,
    /// Workspace of the thread (default $CODEMUX_WORKSPACE_ID).
    #[arg(long, value_name = "ID")]
    pub workspace: Option<String>,
    /// Message, or - to read it from stdin.
    #[arg(long, short, value_name = "MSG")]
    pub message: String,
    /// auto (default) starts an idle thread, steers a busy one or queues;
    /// the others force one of those, or interrupt the current turn.
    #[arg(long, value_enum)]
    pub delivery: Option<Delivery>,
    #[command(flatten)]
    pub run: RunArgs,
    #[command(flatten)]
    pub out: OutputArgs,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delivery {
    Auto,
    Queue,
    Steer,
    Interrupt,
}

impl Delivery {
    fn wire(self) -> &'static str {
        match self {
            Delivery::Auto => "auto",
            Delivery::Queue => "queue",
            Delivery::Steer => "steer",
            Delivery::Interrupt => "interrupt",
        }
    }
}

#[derive(Args)]
pub struct WaitArgs {
    pub thread: String,
    /// Workspace of the thread (default $CODEMUX_WORKSPACE_ID).
    #[arg(long, value_name = "ID")]
    pub workspace: Option<String>,
    /// Follow this turn instead of the thread's current run.
    #[arg(long, value_name = "ID")]
    pub turn: Option<String>,
    /// Stop waiting after this long, e.g. 90s, 10m, 1h (default: until the
    /// run ends or needs input). The work keeps going.
    #[arg(long, value_name = "DURATION", value_parser = args::parse_duration)]
    pub timeout: Option<Duration>,
    #[command(flatten)]
    pub out: OutputArgs,
}

#[derive(Args)]
pub struct ReadArgs {
    pub thread: String,
    /// Workspace of the thread (default $CODEMUX_WORKSPACE_ID).
    #[arg(long, value_name = "ID")]
    pub workspace: Option<String>,
    /// Start after this message id (from a previous page's next_cursor).
    #[arg(long, value_name = "N")]
    pub cursor: Option<i64>,
    /// Messages per page, 1 to 100.
    #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=100))]
    pub limit: u32,
    /// Read every page.
    #[arg(long, conflicts_with = "cursor")]
    pub all: bool,
    #[command(flatten)]
    pub out: OutputArgs,
}

#[derive(Args)]
pub struct ListArgs {
    /// Workspace to list (default $CODEMUX_WORKSPACE_ID).
    #[arg(long, value_name = "ID")]
    pub workspace: Option<String>,
    /// Stop after this many threads (default: all).
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub limit: Option<u32>,
    #[command(flatten)]
    pub out: OutputArgs,
}

#[derive(Args)]
pub struct StatusArgs {
    pub thread: String,
    /// Workspace of the thread (default $CODEMUX_WORKSPACE_ID).
    #[arg(long, value_name = "ID")]
    pub workspace: Option<String>,
    #[command(flatten)]
    pub out: OutputArgs,
}

#[derive(Args)]
pub struct RespondArgs {
    pub thread: String,
    /// The approval's id, from `thread status` or a wait's pending approvals.
    pub request_id: String,
    pub decision: Decision,
    /// Workspace of the thread (default $CODEMUX_WORKSPACE_ID).
    #[arg(long, value_name = "ID")]
    pub workspace: Option<String>,
    /// Idempotency key (default: a new one). The approval's own id is
    /// REQUEST_ID, so this one has its own name.
    #[arg(long, value_name = "ID")]
    pub request_key: Option<String>,
    #[command(flatten)]
    pub out: OutputArgs,
}

#[derive(Args)]
pub struct AnswerArgs {
    pub thread: String,
    /// Workspace of the thread (default $CODEMUX_WORKSPACE_ID).
    #[arg(long, value_name = "ID")]
    pub workspace: Option<String>,
    /// The question's id, from `thread status` or a wait's pending questions.
    #[arg(long, value_name = "ID")]
    pub question: String,
    /// One answer per prompt of the question, in order: repeat the flag.
    #[arg(long = "answer", value_name = "TEXT", required = true)]
    pub answers: Vec<String>,
    /// Idempotency key (default: a new one).
    #[arg(long, value_name = "ID")]
    pub request_id: Option<String>,
    #[command(flatten)]
    pub out: OutputArgs,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    AllowOnce,
    AllowAlways,
    Deny,
}

impl Decision {
    fn wire(self) -> &'static str {
        match self {
            Decision::AllowOnce => "allow_once",
            Decision::AllowAlways => "allow_always",
            Decision::Deny => "deny",
        }
    }
}

#[derive(Args)]
pub struct InterruptArgs {
    pub thread: String,
    /// Workspace of the thread (default $CODEMUX_WORKSPACE_ID).
    #[arg(long, value_name = "ID")]
    pub workspace: Option<String>,
    /// Interrupt only this turn, so a newer run is never cut short.
    #[arg(long, value_name = "ID")]
    pub turn: Option<String>,
    /// Idempotency key (default: a new one).
    #[arg(long, value_name = "ID")]
    pub request_id: Option<String>,
    #[command(flatten)]
    pub out: OutputArgs,
}

#[derive(Args)]
pub struct StopArgs {
    pub thread: String,
    /// Workspace of the thread (default $CODEMUX_WORKSPACE_ID).
    #[arg(long, value_name = "ID")]
    pub workspace: Option<String>,
    /// Idempotency key (default: a new one).
    #[arg(long, value_name = "ID")]
    pub request_id: Option<String>,
    #[command(flatten)]
    pub out: OutputArgs,
}

/// How long a mutation's receipt may take to settle without `--timeout`.
/// It covers a provider starting up.
const SETTLE_BUDGET: Duration = Duration::from_secs(120);

/// One `thread_wait` call. At or below the tool's outside maximum (a unit
/// test checks); each slice is a fresh connection.
pub(crate) const WAIT_SLICE: Duration = Duration::from_secs(60);

/// A follow's first slice: long enough to read its target once, too short
/// to park. A wait does not return for input already pending when it
/// starts, so parking at once would cost a whole slice to report a run that
/// is already blocked. This read reports that run, as the server sees the
/// target, and its cursor carries into the next slice, which then returns
/// for anything that changed after it.
const FIRST_SLICE: Duration = Duration::from_millis(1);

pub(crate) async fn run(door: &Door, command: ThreadCommand) -> Result<Exit, String> {
    match command {
        ThreadCommand::Start(args) => start(door, args).await,
        ThreadCommand::Send(args) => send(door, args).await,
        ThreadCommand::Wait(args) => wait(door, args).await,
        ThreadCommand::Read(args) => read(door, args).await,
        ThreadCommand::List(args) => list(door, args).await,
        ThreadCommand::Status(args) => status(door, args).await,
        ThreadCommand::Respond(args) => respond(door, args).await,
        ThreadCommand::Answer(args) => answer(door, args).await,
        ThreadCommand::Interrupt(args) => interrupt(door, args).await,
        ThreadCommand::Stop(args) => stop(door, args).await,
    }
}

/// `thread_launch` arguments from the flags. Deterministic, so a retry with
/// the same `--request-id` fingerprints the same. Only given flags are sent:
/// the server owns every default.
pub(crate) fn launch_arguments(start: &StartArgs, workspace: &str, message: &str, request_id: &str) -> Value {
    let mut arguments = json!({
        "workspace_id": workspace,
        "client_request_id": request_id,
        "provider": start.provider,
        "message": message,
        "permission_mode": if start.no_permission_mode { Value::Null } else { json!(start.permission_mode) },
    });
    let optional = [("effort", &start.effort), ("title", &start.title)];
    for (key, value) in optional {
        if let Some(value) = value {
            arguments[key] = json!(value);
        }
    }
    // An instance carries its own model, so the two never disagree.
    match &start.instance {
        Some(instance) => {
            let mut target = json!({"instance_id": instance});
            if let Some(model) = &start.model {
                target["model"] = json!(model);
            }
            arguments["target"] = target;
        }
        None => {
            if let Some(model) = &start.model {
                arguments["model"] = json!(model);
            }
        }
    }
    if let Some(branch) = &start.worktree_branch {
        let mut worktree = json!({"branch": branch});
        if let Some(base) = &start.worktree_base {
            worktree["base"] = json!(base);
        }
        arguments["worktree"] = worktree;
    }
    arguments
}

async fn start(door: &Door, start: StartArgs) -> Result<Exit, String> {
    let json = start.out.json;
    let request_id = args::request_id(start.run.request_id.as_deref());
    let prepared = args::workspace(start.workspace.as_deref())
        .and_then(|workspace| Ok((args::message(&start.message)?, workspace)));
    let (message, workspace) = match prepared {
        Ok(prepared) => prepared,
        Err(failure) if start.run.wait => return print_report(RunReport::ended(failure, Some(&request_id)), json),
        Err(failure) => return output::report(verbs::THREAD_LAUNCH, Err(failure), json, None, |_| Ok(())),
    };
    let arguments = launch_arguments(&start, &workspace, &message, &request_id);
    let deadline = start.run.timeout.map(|timeout| Instant::now() + timeout);
    let settled = mutate(door, verbs::THREAD_LAUNCH, arguments, deadline).await;
    if !start.run.wait {
        return output::report(verbs::THREAD_LAUNCH, settled, json, Some(&request_id), |receipt| {
            let thread = receipt["thread_id"].as_str().unwrap_or_default();
            output::out(thread)?;
            output::note(&format!("Started thread {thread} in workspace {}.", receipt["workspace_id"].as_str().unwrap_or(&workspace)));
            Ok(())
        });
    }
    let report = match followed(settled, &workspace, None, &request_id) {
        Ok(run) => {
            if !json {
                output::note(&format!("Thread {} (workspace {})", run.thread_id, run.workspace_id));
            }
            follow_run(door, run, deadline, true).await
        }
        Err(report) => *report,
    };
    print_report(report, json)
}

/// Calls a mutation and settles its receipt within `deadline` (default
/// [`SETTLE_BUDGET`]). The call itself is the verb's first, so it is never
/// retried, and `deadline` bounds it too.
async fn mutate(door: &Door, tool: &str, arguments: Value, deadline: Option<Instant>) -> Result<Value, CliError> {
    let receipt = door::by_deadline(deadline, door.call(tool, arguments)).await?;
    door::settle(door, receipt, deadline.unwrap_or_else(|| Instant::now() + SETTLE_BUDGET)).await
}

/// What `--wait` follows after a launch or send: the receipt's thread (else
/// `thread`) in the receipt's workspace (a worktree launch names its new
/// one), and the turn its message started or joined. A queued message has no
/// turn yet: its operation names the run once it starts.
fn followed(settled: Result<Value, CliError>, workspace: &str, thread: Option<&str>, request_id: &str) -> Result<Followed, Box<RunReport>> {
    let receipt = settled.map_err(|failure| Box::new(RunReport::ended(failure, Some(request_id))))?;
    let text = |value: &Value| value.as_str().filter(|text| !text.is_empty()).map(str::to_owned);
    let result = &receipt["result"];
    let run = Followed {
        workspace_id: text(&receipt["workspace_id"]).or_else(|| text(&result["workspace_id"])).unwrap_or_else(|| workspace.to_owned()),
        thread_id: text(&receipt["thread_id"])
            .or_else(|| text(&result["thread_id"]))
            .or_else(|| thread.map(str::to_owned))
            .unwrap_or_default(),
        turn_id: text(&result["turn"]["turn_id"]),
        operation_id: text(&receipt["operation_id"]),
        client_request_id: Some(request_id.to_owned()),
    };
    if !output::succeeded(&receipt) || run.thread_id.is_empty() {
        let mut report = RunReport::of(&run);
        report.error = Some(if output::succeeded(&receipt) {
            ErrorBody::new(code::UNCODED, "The receipt names no thread to follow")
        } else {
            output::receipt_error(&receipt)
        });
        return Err(Box::new(report));
    }
    Ok(run)
}

async fn send(door: &Door, send: SendArgs) -> Result<Exit, String> {
    let json = send.out.json;
    let request_id = args::request_id(send.run.request_id.as_deref());
    let prepared = args::workspace(send.workspace.as_deref())
        .and_then(|workspace| Ok((args::message(&send.message)?, workspace)));
    let (message, workspace) = match prepared {
        Ok(prepared) => prepared,
        Err(failure) if send.run.wait => return print_report(RunReport::ended(failure, Some(&request_id)), json),
        Err(failure) => return output::report(verbs::THREAD_SEND, Err(failure), json, None, |_| Ok(())),
    };
    let mut arguments = json!({
        "workspace_id": workspace,
        "thread_id": send.thread,
        "client_request_id": request_id,
        "message": message,
    });
    if let Some(delivery) = send.delivery {
        arguments["delivery"] = json!(delivery.wire());
    }
    let deadline = send.run.timeout.map(|timeout| Instant::now() + timeout);
    let settled = mutate(door, verbs::THREAD_SEND, arguments, deadline).await;
    if !send.run.wait {
        return output::report(verbs::THREAD_SEND, settled, json, Some(&request_id), |receipt| {
            output::out(match receipt["result"]["accepted_as"].as_str() {
                Some("queued") => "Queued.",
                Some("steered") => "Steered.",
                _ => "Sent.",
            })
        });
    }
    let report = match followed(settled, &workspace, Some(&send.thread), &request_id) {
        Ok(run) => follow_run(door, run, deadline, true).await,
        Err(report) => *report,
    };
    print_report(report, json)
}

async fn wait(door: &Door, wait: WaitArgs) -> Result<Exit, String> {
    let json = wait.out.json;
    let workspace = match args::workspace(wait.workspace.as_deref()) {
        Ok(workspace) => workspace,
        Err(failure) => return print_report(RunReport::ended(failure, None), json),
    };
    let run = Followed { workspace_id: workspace, thread_id: wait.thread, turn_id: wait.turn, operation_id: None, client_request_id: None };
    let deadline = wait.timeout.map(|timeout| Instant::now() + timeout);
    print_report(follow_run(door, run, deadline, false).await, json)
}

async fn read(door: &Door, read: ReadArgs) -> Result<Exit, String> {
    let outcome = async {
        let mut arguments = json!({"workspace_id": args::workspace(read.workspace.as_deref())?, "thread_id": read.thread, "limit": read.limit});
        if let Some(cursor) = read.cursor {
            arguments["cursor"] = json!(cursor);
        }
        let mut page = door.call(verbs::THREAD_READ, arguments.clone()).await?;
        // `--all` follows next_cursor and returns one merged page.
        while read.all {
            let Some(cursor) = page["next_cursor"].as_i64() else { break };
            arguments["cursor"] = json!(cursor);
            let next = door.call_in_loop(verbs::THREAD_READ, arguments.clone()).await?;
            let more = next["messages"].as_array().cloned().unwrap_or_default();
            if let Some(messages) = page["messages"].as_array_mut() {
                messages.extend(more);
            }
            page["next_cursor"] = next["next_cursor"].clone();
        }
        Ok(page)
    }
    .await;
    output::report(verbs::THREAD_READ, outcome, read.out.json, None, |page| {
        for message in page["messages"].as_array().into_iter().flatten() {
            output::out(&format!("[{}] {}\n", message["role"].as_str().unwrap_or("?"), message["content"].as_str().unwrap_or_default()))?;
        }
        if let Some(cursor) = page["next_cursor"].as_i64() {
            output::note(&format!("More: --cursor {cursor}"));
        }
        Ok(())
    })
}

/// thread_list's own page limit.
const LIST_PAGE: u32 = 100;

async fn list(door: &Door, list: ListArgs) -> Result<Exit, String> {
    let outcome = async {
        let workspace = args::workspace(list.workspace.as_deref())?;
        let wanted = list.limit.map_or(usize::MAX, |limit| limit as usize);
        let mut threads: Vec<Value> = Vec::new();
        let mut cursor: Option<String> = None;
        let mut has_more = true;
        while has_more && threads.len() < wanted {
            let mut arguments = json!({"workspace_id": workspace, "limit": LIST_PAGE});
            if let Some(cursor) = &cursor {
                arguments["cursor"] = json!(cursor);
            }
            let page = if cursor.is_none() {
                door.call(verbs::THREAD_LIST, arguments).await?
            } else {
                door.call_in_loop(verbs::THREAD_LIST, arguments).await?
            };
            threads.extend(page["threads"].as_array().cloned().unwrap_or_default());
            cursor = page["next_cursor"].as_str().map(str::to_owned);
            has_more = page["has_more"] == true && cursor.is_some();
        }
        // A limit that cut a page short leaves more to read after the last kept thread.
        if threads.len() > wanted {
            threads.truncate(wanted);
            has_more = true;
        }
        let next_cursor = has_more.then(|| threads.last().and_then(|thread| thread["thread_id"].as_str()).map(str::to_owned)).flatten();
        Ok(json!({"threads": threads, "has_more": has_more, "next_cursor": next_cursor}))
    }
    .await;
    output::report(verbs::THREAD_LIST, outcome, list.out.json, None, |page| {
        let mut rows = vec![vec!["THREAD".into(), "PROVIDER".into(), "TITLE".into()]];
        for thread in page["threads"].as_array().into_iter().flatten() {
            let cell = |key: &str| thread[key].as_str().unwrap_or_default().to_owned();
            rows.push(vec![cell("thread_id"), cell("provider"), cell("title")]);
        }
        output::table(&rows)
    })
}

async fn status(door: &Door, status: StatusArgs) -> Result<Exit, String> {
    let outcome = async {
        let workspace = args::workspace(status.workspace.as_deref())?;
        door.call(verbs::THREAD_STATUS, json!({"workspace_id": workspace, "thread_id": status.thread})).await
    }
    .await;
    output::report(verbs::THREAD_STATUS, outcome, status.out.json, None, |status| {
        let text = |value: &Value| value.as_str().unwrap_or("?").to_owned();
        output::out(&format!("{}  phase={}  settled={}", text(&status["state"]), text(&status["phase"]), status["settled"]))?;
        for line in pending_lines(&status["pending"]) {
            output::out(&line)?;
        }
        Ok(())
    })
}

async fn respond(door: &Door, respond: RespondArgs) -> Result<Exit, String> {
    let request_key = args::request_id(respond.request_key.as_deref());
    let outcome = async {
        let arguments = json!({
            "workspace_id": args::workspace(respond.workspace.as_deref())?,
            "thread_id": respond.thread,
            "request_id": respond.request_id,
            "decision": respond.decision.wire(),
            "client_request_id": request_key,
        });
        mutate(door, verbs::THREAD_RESPOND, arguments, None).await
    }
    .await;
    output::report(verbs::THREAD_RESPOND, outcome, respond.out.json, Some(&request_key), |_| output::out("Done."))
}

/// A failed answer exits 1, also when a retry of its key returns the failed
/// receipt instead of an error.
async fn answer(door: &Door, answer: AnswerArgs) -> Result<Exit, String> {
    let request_id = args::request_id(answer.request_id.as_deref());
    let outcome = async {
        let arguments = json!({
            "workspace_id": args::workspace(answer.workspace.as_deref())?,
            "thread_id": answer.thread,
            "client_request_id": request_id,
            "question_id": answer.question,
            "answers": answer.answers,
        });
        mutate(door, verbs::THREAD_ANSWER, arguments, None).await
    }
    .await;
    output::report(verbs::THREAD_ANSWER, outcome, answer.out.json, Some(&request_id), |answered| {
        // The tool's own result, or the receipt a settled retry read.
        let field = |key: &str| answered[key].as_str().or_else(|| answered["result"][key].as_str()).unwrap_or_default();
        let turn = field("turn_id");
        output::out(turn)?;
        output::note(&format!("Answered. Follow the run: codemux thread wait {} --turn {turn} --workspace {}", field("thread_id"), field("workspace_id")));
        Ok(())
    })
}

async fn interrupt(door: &Door, interrupt: InterruptArgs) -> Result<Exit, String> {
    let request_id = args::request_id(interrupt.request_id.as_deref());
    let outcome = async {
        let mut arguments = json!({
            "workspace_id": args::workspace(interrupt.workspace.as_deref())?,
            "thread_id": interrupt.thread,
            "client_request_id": request_id,
        });
        if let Some(turn) = &interrupt.turn {
            arguments["turn_id"] = json!(turn);
        }
        mutate(door, verbs::THREAD_INTERRUPT, arguments, None).await
    }
    .await;
    output::report(verbs::THREAD_INTERRUPT, outcome, interrupt.out.json, Some(&request_id), |_| output::out("Done."))
}

async fn stop(door: &Door, stop: StopArgs) -> Result<Exit, String> {
    let request_id = args::request_id(stop.request_id.as_deref());
    let outcome = async {
        let arguments = json!({
            "workspace_id": args::workspace(stop.workspace.as_deref())?,
            "thread_id": stop.thread,
            "client_request_id": request_id,
        });
        mutate(door, verbs::THREAD_STOP, arguments, None).await
    }
    .await;
    output::report(verbs::THREAD_STOP, outcome, stop.out.json, Some(&request_id), |_| output::out("Done."))
}

// ── Following a run ───────────────────────────────────────────────────────

/// The run `--wait` and `thread wait` follow.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Followed {
    pub(crate) workspace_id: String,
    pub(crate) thread_id: String,
    /// Follow this turn. Without one, follow the run `operation_id`'s
    /// message starts (a queued message's), else the thread's current run.
    pub(crate) turn_id: Option<String>,
    pub(crate) operation_id: Option<String>,
    pub(crate) client_request_id: Option<String>,
}

/// How a followed run ended, as far as the CLI waited.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RunState {
    Done,
    Failed,
    NeedsInput,
    TimedOut,
}

impl RunState {
    pub(crate) fn exit(self) -> Exit {
        match self {
            RunState::Done => Exit::Done,
            RunState::Failed => Exit::Failed,
            RunState::NeedsInput => Exit::NeedsInput,
            RunState::TimedOut => Exit::TimedOut,
        }
    }
}

/// What `--wait` and `thread wait` print. `ok` is true exactly when the exit
/// status is 0. Printed even on failure, so a script always learns the thread.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub(crate) struct RunReport {
    pub(crate) ok: bool,
    pub(crate) state: RunState,
    /// Why a failed run failed (`error`, `interrupted`, `no_final_message`, …).
    pub(crate) failure: Option<String>,
    pub(crate) workspace_id: Option<String>,
    pub(crate) thread_id: Option<String>,
    pub(crate) operation_id: Option<String>,
    pub(crate) client_request_id: Option<String>,
    pub(crate) turn_id: Option<String>,
    /// The run's last assistant message. Untrusted content.
    pub(crate) final_text: Option<String>,
    /// `{approvals, questions, secrets}` from the last wait.
    pub(crate) pending: Option<Value>,
    pub(crate) error: Option<ErrorBody>,
}

impl RunReport {
    fn of(run: &Followed) -> Self {
        Self {
            ok: false,
            state: RunState::Failed,
            failure: None,
            workspace_id: Some(run.workspace_id.clone()),
            thread_id: Some(run.thread_id.clone()),
            operation_id: run.operation_id.clone(),
            client_request_id: run.client_request_id.clone(),
            turn_id: run.turn_id.clone(),
            final_text: None,
            pending: None,
            error: None,
        }
    }

    /// A run that could not be followed at all.
    fn ended(failure: CliError, request_id: Option<&str>) -> Self {
        let last = failure.last.clone().unwrap_or(Value::Null);
        let text = |key: &str| last[key].as_str().map(str::to_owned);
        Self {
            ok: false,
            state: RunState::Failed,
            failure: None,
            workspace_id: text("workspace_id"),
            thread_id: text("thread_id"),
            operation_id: text("operation_id"),
            client_request_id: request_id.map(str::to_owned),
            turn_id: None,
            final_text: None,
            pending: None,
            error: None,
        }
        .stopped(failure)
    }

    /// Ends the report on a call that failed: a deadline is `timed_out`,
    /// anything else `failed`.
    fn stopped(mut self, failure: CliError) -> Self {
        self.state = if failure.exit == Exit::TimedOut { RunState::TimedOut } else { RunState::Failed };
        self.error = Some(failure.error);
        self.ok = false;
        self
    }

    fn finish(mut self, state: RunState) -> Self {
        self.state = state;
        self.ok = state == RunState::Done;
        self
    }

    /// Takes in what one `thread_wait` reported.
    fn absorb(&mut self, waited: &Value) {
        let text = |key: &str| waited[key].as_str().map(str::to_owned);
        if let Some(turn) = text("turn_id") {
            self.turn_id = Some(turn);
        }
        self.failure = text("failure");
        self.final_text = text("final_text");
        self.pending = Some(waited["pending"].clone()).filter(|pending| !pending.is_null());
    }
}

/// Wait states the server reports (`wait.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Waited {
    Running,
    NeedsInput,
    Done,
    Failed,
}

fn wait_state(waited: &Value) -> Result<Waited, ErrorBody> {
    match waited["state"].as_str() {
        Some("running") => Ok(Waited::Running),
        Some("needs_input") => Ok(Waited::NeedsInput),
        Some("done") => Ok(Waited::Done),
        Some("failed") => Ok(Waited::Failed),
        other => Err(ErrorBody::new(
            code::UNEXPECTED_STATE,
            format!("thread_wait reported state {}, which this codemux CLI does not know; update it", other.map_or_else(|| "null".to_owned(), |state| format!("`{state}`"))),
        )),
    }
}

/// Whether a thread status object still lists queue entries. Uncertain
/// entries count: their delivery is unsettled.
fn queued(status: &Value) -> bool {
    ["queued_ids", "uncertain_queued_ids"].iter().any(|key| status[*key].as_array().is_some_and(|ids| !ids.is_empty()))
}

/// What a follow waits for.
#[derive(Debug, Clone, PartialEq)]
enum Target {
    /// One run, named by a receipt or by `--turn`. The follow ends when this
    /// run does, whatever else is queued behind it: those are other
    /// messages' runs.
    Run(String),
    /// The run a queued message starts. `thread_wait` resolves the
    /// operation to that run once it starts, and reports input that holds
    /// the queue meanwhile.
    Operation(String),
    /// The thread's current run, then whatever is queued behind it.
    Thread,
}

impl Target {
    fn of(run: &Followed) -> Self {
        match (&run.turn_id, &run.operation_id) {
            (Some(turn), _) => Target::Run(turn.clone()),
            (None, Some(operation)) => Target::Operation(operation.clone()),
            (None, None) => Target::Thread,
        }
    }

    /// Selects this target in `thread_wait` arguments.
    fn select(&self, arguments: &mut Value) {
        match self {
            Target::Run(turn) => arguments["turn_id"] = json!(turn),
            Target::Operation(operation) => arguments["operation_id"] = json!(operation),
            Target::Thread => {}
        }
    }

    /// Whether an ended run leaves this follow waiting: a thread with queued
    /// work. A delivery that needs the person never drains on its own, so it
    /// ends the wait.
    fn outlives(&self, waited: &Value) -> bool {
        *self == Target::Thread && waited["failure"] != "delivery_needs_attention" && queued(&waited["status"])
    }
}

/// One `thread_wait` of the follow loop, given up at `deadline`. Retried,
/// except as the verb's first call, so a stopped app fails fast.
async fn follow_call(door: &Door, retry: bool, arguments: Value, deadline: Option<Instant>) -> Result<Value, CliError> {
    door::by_deadline(deadline, async {
        if retry {
            door.call_in_loop(verbs::THREAD_WAIT, arguments).await
        } else {
            door.call(verbs::THREAD_WAIT, arguments).await
        }
    })
    .await
}

/// Follows a run with `thread_wait` slices until it is done, failed or needs
/// input, or `deadline` passes, which also cuts off a call still under way.
/// The first slice only reads the target ([`FIRST_SLICE`]); each later one
/// passes the last `wait_cursor` as `since`, so it parks until something
/// changes. The server reads every slice for the target, so input another
/// run waits on never ends this follow. Never interrupts or stops anything.
/// `retry_first` is false when the first call is the verb's first, so a
/// stopped app fails fast.
pub(crate) async fn follow_run(door: &Door, run: Followed, deadline: Option<Instant>, retry_first: bool) -> RunReport {
    let mut report = RunReport::of(&run);
    let target = Target::of(&run);
    let thread = json!({"workspace_id": run.workspace_id, "thread_id": run.thread_id});
    let mut since: Option<String> = None;
    let mut retry = retry_first;
    let mut longest = FIRST_SLICE;
    loop {
        // Sized from what is left now, after the last call took its time.
        let slice = match deadline {
            None => longest,
            Some(deadline) => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left < Duration::from_millis(1) {
                    return report.stopped(CliError::new(
                        Exit::TimedOut,
                        code::TIMED_OUT,
                        "The run was still going at the deadline. Nothing was cancelled.",
                    ));
                }
                left.min(longest)
            }
        };
        let mut arguments = thread.clone();
        arguments["timeout_ms"] = json!(slice.as_millis() as u64);
        target.select(&mut arguments);
        if let Some(since) = &since {
            arguments["since"] = json!(since);
        }
        let waited = match follow_call(door, retry, arguments, deadline).await {
            Ok(waited) => waited,
            Err(failure) => return report.stopped(failure),
        };
        retry = true;
        longest = WAIT_SLICE;
        // A message that never ran fails with its receipt's own error.
        if matches!(waited["operation"]["state"].as_str(), Some("failed" | "needs_attention")) {
            return RunReport::of(&run).stopped(CliError::from(output::receipt_error(&waited["operation"])));
        }
        report.absorb(&waited);
        let state = match wait_state(&waited) {
            Ok(state) => state,
            Err(error) => return report.stopped(error.into()),
        };
        since = waited["wait_cursor"].as_str().map(str::to_owned);
        match state {
            Waited::Running => {}
            Waited::NeedsInput => return report.finish(RunState::NeedsInput),
            Waited::Done | Waited::Failed if target.outlives(&waited) => {}
            Waited::Done => return report.finish(RunState::Done),
            Waited::Failed => return report.finish(RunState::Failed),
        }
    }
}

/// Prints a run report and returns its exit status.
fn print_report(report: RunReport, json: bool) -> Result<Exit, String> {
    let exit = report.state.exit();
    if json {
        output::emit_json(&report)?;
        return Ok(exit);
    }
    let thread = report.thread_id.as_deref().unwrap_or_default();
    let workspace = report.workspace_id.as_deref().unwrap_or("<workspace>");
    match report.state {
        RunState::Done => {
            if let Some(text) = &report.final_text {
                output::out(text)?;
            }
        }
        RunState::NeedsInput => {
            for line in pending_lines(report.pending.as_ref().unwrap_or(&Value::Null)) {
                output::note(&format!("Thread {thread} is waiting: {line}"));
            }
            if let Some(approval) = report.pending.as_ref().and_then(|pending| pending["approvals"][0]["request_id"].as_str()) {
                output::note(&format!(
                    "Answer: codemux thread respond {thread} {approval} allow-once|deny --workspace {workspace}"
                ));
            }
            if let Some(question) = report.pending.as_ref().and_then(|pending| pending["questions"][0]["question_id"].as_str()) {
                output::note(&format!(
                    "Answer: codemux thread answer {thread} --question {question} --answer <text> --workspace {workspace}"
                ));
            }
        }
        // A launch the deadline cut off before Codemux answered names no
        // thread to wait on yet.
        RunState::TimedOut if thread.is_empty() => {
            if let Some(error) = &report.error {
                output::note(&error.to_string());
            }
        }
        RunState::TimedOut => output::note(&format!(
            "Still running at the deadline. Wait again: codemux thread wait {thread} --workspace {workspace}"
        )),
        RunState::Failed if thread.is_empty() => {
            if let Some(error) = &report.error {
                output::note(&error.to_string());
            }
        }
        RunState::Failed => {
            let why = match (&report.failure, &report.error) {
                (_, Some(error)) => error.to_string(),
                (Some(failure), None) => failure.clone(),
                (None, None) => "failed".to_owned(),
            };
            output::note(&format!("Thread {thread} ended: failed: {why}"));
        }
    }
    if let (Some(error), Some(key)) = (&report.error, &report.client_request_id) {
        if report.state == RunState::Failed {
            output::retry_hint(&CliError::from(error.clone()), key);
        }
    }
    Ok(exit)
}

/// One line per pending approval, question and secret card.
fn pending_lines(pending: &Value) -> Vec<String> {
    let items = |kind: &str| pending[kind].as_array().cloned().unwrap_or_default();
    let text = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    let mut lines = Vec::new();
    for approval in items("approvals") {
        lines.push(format!("approval {} ({})", text(&approval["request_id"]), text(&approval["request_kind"])));
    }
    for question in items("questions") {
        lines.push(format!("question {}: {} (answer it in Codemux or with `codemux thread answer`)", text(&question["question_id"]), text(&question["text"])));
    }
    for secret in items("secrets") {
        lines.push(format!("secret {} \"{}\" (enter it in Codemux)", text(&secret["request_id"]), text(&secret["label"])));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, CommandSet};
    use clap::Parser;

    fn start_args(argv: &[&str]) -> StartArgs {
        let mut full = vec!["codemux", "thread", "start"];
        full.extend_from_slice(argv);
        match Cli::try_parse_from(full).expect("valid argv").command {
            Some(CommandSet::Thread { command: ThreadCommand::Start(args) }) => args,
            _ => panic!("expected thread start"),
        }
    }

    fn base() -> Vec<&'static str> {
        vec!["--workspace", "ws-flag", "--provider", "codex", "--permission-mode", "read-only", "-m", "hi"]
    }

    fn receipt(state: &str, result: Value) -> Value {
        json!({"operation_id": "op-1", "tool": "thread_launch", "state": state, "workspace_id": "ws-new",
            "thread_id": "th-1", "result": result, "error": null})
    }

    fn waited(state: &str, extra: Value) -> Value {
        let mut value = json!({"timed_out": false, "settled": state != "running", "state": state, "turn_id": "t1",
            "failure": null, "final_text": null, "pending": {"approvals": [], "questions": [], "secrets": []},
            "status": {"queued_ids": [], "uncertain_queued_ids": []}, "wait_cursor": format!("cur-{state}")});
        for (key, field) in extra.as_object().cloned().unwrap_or_default() {
            value[key] = field;
        }
        value
    }

    /// A `thread_status` reply: the legacy status object plus the current
    /// run's state.
    fn status_reply(state: &str, extra: Value) -> Value {
        let mut value = json!({"workspace_id": "ws", "thread_id": "th", "phase": "running", "settled": false, "turn_id": "t1",
            "queued_ids": [], "uncertain_queued_ids": [], "state": state, "failure": null,
            "pending": {"approvals": [], "questions": [], "secrets": []}, "paused_until_ms": null, "wait_cursor": format!("st-{state}")});
        for (key, field) in extra.as_object().cloned().unwrap_or_default() {
            value[key] = field;
        }
        value
    }

    /// The tool every follow reads first: a wait that only reads.
    const FIRST_READ: &str = "thread_wait";

    /// The read every follow starts with, for a run still going.
    fn busy() -> (&'static str, Result<Value, CliError>) {
        (FIRST_READ, Ok(waited("running", json!({"wait_cursor": "cur-first"}))))
    }

    /// A call to an app that took the request and never answers: the door
    /// reports no reply once the call's own bound has passed.
    fn stalled(name: &str) -> (&str, Duration, Result<Value, CliError>) {
        (name, Duration::from_secs(120), Err(CliError::failed(code::NO_REPLY, "Codemux sent no reply within 120s.")))
    }

    fn quick((name, answer): (&'static str, Result<Value, CliError>)) -> (&'static str, Duration, Result<Value, CliError>) {
        (name, Duration::ZERO, answer)
    }

    async fn follow(door: &Door, deadline: Option<Instant>) -> RunReport {
        follow_run(door, following(None, None), deadline, false).await
    }

    fn following(turn: Option<&str>, operation: Option<&str>) -> Followed {
        Followed {
            workspace_id: "ws".into(),
            thread_id: "th".into(),
            turn_id: turn.map(str::to_owned),
            operation_id: operation.map(str::to_owned),
            client_request_id: None,
        }
    }

    #[test]
    fn launch_arguments_are_deterministic() {
        let args = start_args(&base());
        let first = launch_arguments(&args, "ws", "hi", "key-1");
        assert_eq!(serde_json::to_string(&first).unwrap(), serde_json::to_string(&launch_arguments(&args, "ws", "hi", "key-1")).unwrap());
        assert_eq!(first, json!({"workspace_id": "ws", "client_request_id": "key-1", "provider": "codex", "message": "hi", "permission_mode": "read-only"}));

        // No mode is sent as null only when asked; the CLI never picks one.
        let mut argv = base();
        argv.splice(4..6, ["--no-permission-mode"]);
        assert_eq!(launch_arguments(&start_args(&argv), "ws", "hi", "k")["permission_mode"], Value::Null);
        assert!(launch_arguments(&start_args(&argv), "ws", "hi", "k").get("permission_mode").is_some());

        // An instance carries the model inside its target.
        let mut argv = base();
        argv.extend(["--instance", "codex:work", "--model", "gpt-x", "--effort", "low", "--title", "T"]);
        let with_instance = launch_arguments(&start_args(&argv), "ws", "hi", "k");
        assert_eq!(with_instance["target"], json!({"instance_id": "codex:work", "model": "gpt-x"}));
        assert!(with_instance.get("model").is_none());
        assert_eq!((with_instance["effort"].as_str(), with_instance["title"].as_str()), (Some("low"), Some("T")));
        let mut argv = base();
        argv.extend(["--model", "gpt-x"]);
        assert_eq!(launch_arguments(&start_args(&argv), "ws", "hi", "k")["model"], "gpt-x");

        let mut argv = base();
        argv.extend(["--worktree-branch", "fix-1", "--worktree-base", "trunk"]);
        assert_eq!(launch_arguments(&start_args(&argv), "ws", "hi", "k")["worktree"], json!({"branch": "fix-1", "base": "trunk"}));
    }

    #[tokio::test(start_paused = true)]
    async fn start_without_wait_settles_the_receipt() {
        let door = Door::fake(vec![
            ("thread_launch", Ok(receipt("accepted", Value::Null))),
            ("operation_status", Ok(receipt("running", Value::Null))),
            ("operation_status", Ok(receipt("succeeded", json!({"turn": {"turn_id": "t1"}})))),
        ]);
        let exit = start(&door, start_args(&[base().as_slice(), &["--json", "--request-id", "k"]].concat())).await.unwrap();
        assert_eq!(exit, Exit::Done);
        assert!(door.script().finished());
        assert_eq!(door.script().calls()[0].1["client_request_id"], "k");
        assert_eq!(door.script().calls()[1].1, json!({"operation_id": "op-1"}));
    }

    #[tokio::test(start_paused = true)]
    async fn start_wait_follows_the_receipts_run() {
        let door = Door::fake(vec![
            ("thread_launch", Ok(receipt("succeeded", json!({"turn": {"turn_id": "t1", "queued_id": null, "steered": false}})))),
            busy(),
            ("thread_wait", Ok(waited("running", json!({})))),
            ("thread_wait", Ok(waited("done", json!({"final_text": "PONG"})))),
        ]);
        let mut argv = base();
        argv.extend(["--wait", "--json", "--request-id", "k"]);
        let exit = start(&door, start_args(&argv)).await.unwrap();
        assert_eq!(exit, Exit::Done);
        let calls = door.script().calls();
        // The receipt's workspace (a worktree launch names its new one), not the flag's.
        // The first slice only reads the run; the next ones park until the
        // view moves on from the last one.
        assert_eq!(calls[1].1, json!({"workspace_id": "ws-new", "thread_id": "th-1", "turn_id": "t1", "timeout_ms": 1}));
        assert_eq!(calls[2].1, json!({"workspace_id": "ws-new", "thread_id": "th-1", "turn_id": "t1", "timeout_ms": 60_000, "since": "cur-first"}));
        assert_eq!(calls[3].1["since"], "cur-running");
        assert_eq!(calls[3].1["turn_id"], "t1");

        let run = Followed { operation_id: Some("op-1".into()), client_request_id: Some("k".into()), ..following(Some("t1"), None) };
        let door = Door::fake(vec![busy(), ("thread_wait", Ok(waited("done", json!({"final_text": "PONG"}))))]);
        let report = follow_run(&door, run, None, true).await;
        assert_eq!((report.ok, report.state, report.final_text.as_deref()), (true, RunState::Done, Some("PONG")));
        assert_eq!((report.operation_id.as_deref(), report.client_request_id.as_deref()), (Some("op-1"), Some("k")));
    }

    #[tokio::test(start_paused = true)]
    async fn start_wait_follows_a_queued_first_message_by_its_operation() {
        // The provider queued the launch's message, so the settled receipt
        // names only the queue entry and never changes. The follow waits on
        // the operation, which the server resolves to the message's run.
        let launched = receipt("succeeded", json!({"turn": {"turn_id": "", "queued_id": "q", "steered": false}}));
        let operation = json!({"operation_id": "op-1", "state": "succeeded", "error": null});
        let door = Door::fake(vec![
            ("thread_launch", Ok(launched)),
            ("thread_wait", Ok(waited("running", json!({"turn_id": null, "operation": operation, "timed_out": true})))),
            ("thread_wait", Ok(waited("running", json!({"turn_id": "t1", "operation": operation, "wait_cursor": "cur-started"})))),
            ("thread_wait", Ok(waited("done", json!({"turn_id": "t1", "final_text": "PONG", "operation": operation})))),
        ]);
        let mut argv = base();
        argv.extend(["--wait", "--json", "--request-id", "k"]);
        assert_eq!(start(&door, start_args(&argv)).await.unwrap(), Exit::Done);
        assert!(door.script().finished());
        let calls = door.script().calls();
        assert_eq!(calls[1].1, json!({"workspace_id": "ws-new", "thread_id": "th-1", "operation_id": "op-1", "timeout_ms": 1}));
        assert_eq!((calls[2].1["operation_id"].as_str(), calls[2].1["since"].as_str()), (Some("op-1"), Some("cur-running")));
        assert_eq!((calls[3].1["operation_id"].as_str(), calls[3].1["since"].as_str()), (Some("op-1"), Some("cur-started")));
        assert!(calls.iter().all(|(_, arguments)| arguments.get("turn_id").is_none()), "the server picks the run");
    }

    #[tokio::test(start_paused = true)]
    async fn needs_input_exits_2_with_pending() {
        let approval = json!({"approvals": [{"request_id": "r1", "turn_id": "t1", "request_kind": "command"}], "questions": [], "secrets": []});
        let door = Door::fake(vec![busy(), ("thread_wait", Ok(waited("needs_input", json!({"pending": approval}))))]);
        let report = follow(&door, None).await;
        assert_eq!((report.state, report.state.exit().code(), report.ok), (RunState::NeedsInput, 2, false));
        assert_eq!(report.pending, Some(approval));
        assert_eq!(pending_lines(report.pending.as_ref().unwrap()), ["approval r1 (command)"]);
    }

    #[tokio::test(start_paused = true)]
    async fn input_already_pending_is_reported_without_parking() {
        // A wait does not return for input pending when it starts, so the
        // first slice, which only reads, reports it before any slice parks.
        let approval = json!({"approvals": [{"request_id": "r1", "turn_id": "t1", "request_kind": "command"}], "questions": [], "secrets": []});
        let blocked = || (FIRST_READ, Ok(waited("needs_input", json!({"pending": approval, "timed_out": true}))));
        for run in [following(None, None), following(Some("t1"), None), following(None, Some("op"))] {
            let door = Door::fake(vec![blocked()]);
            let report = follow_run(&door, run.clone(), None, false).await;
            assert_eq!((report.state, report.turn_id.as_deref(), report.pending.as_ref()), (RunState::NeedsInput, Some("t1"), Some(&approval)), "{run:?}");
            let calls = door.script().calls();
            assert_eq!((calls.len(), calls[0].1["timeout_ms"].as_u64(), calls[0].1.get("since")), (1, Some(1), None), "{run:?}");
        }

        // The server reads that slice for the target, so input a newer run
        // waits on does not stop the follow of an older one.
        let door = Door::fake(vec![(FIRST_READ, Ok(waited("done", json!({"turn_id": "t0", "final_text": "older", "pending": approval}))))]);
        let report = follow_run(&door, following(Some("t0"), None), None, false).await;
        assert_eq!((report.state, report.final_text.as_deref()), (RunState::Done, Some("older")));
        assert_eq!(door.script().calls()[0].1["turn_id"], "t0");
    }

    #[tokio::test(start_paused = true)]
    async fn a_retried_launch_follows_its_own_run_not_a_newer_runs_input() {
        // A keyed retry of a launch returns its settled receipt again. Its
        // first message was queued, so the receipt names only the queue
        // entry. That message has since run and finished as t1, and a newer
        // run in the thread waits on an approval. The follow asks for the
        // operation's run and never reads the thread's own input.
        let retried = receipt("succeeded", json!({"accepted_as": "queued", "turn": {"turn_id": "", "queued_id": "q", "steered": false}}));
        let operation = json!({"operation_id": "op-1", "state": "succeeded", "error": null});
        let door = Door::fake(vec![
            ("thread_launch", Ok(retried)),
            (FIRST_READ, Ok(waited("done", json!({"turn_id": "t1", "final_text": "PONG", "operation": operation})))),
        ]);
        let mut argv = base();
        argv.extend(["--wait", "--json", "--request-id", "k"]);
        assert_eq!(start(&door, start_args(&argv)).await.unwrap(), Exit::Done);
        assert_eq!(door.script().names(), ["thread_launch", "thread_wait"], "no read of the whole thread");
        assert_eq!(door.script().calls()[1].1, json!({"workspace_id": "ws-new", "thread_id": "th-1", "operation_id": "op-1", "timeout_ms": 1}));
    }

    #[tokio::test(start_paused = true)]
    async fn the_first_parked_wait_starts_from_the_first_reads_cursor() {
        // Input that opens after the first read but before the next slice
        // starts would be that slice's baseline without `since`, and come
        // out only at its timeout. With the first read's cursor it is a
        // change, so the slice returns it at once.
        let approval = json!({"approvals": [{"request_id": "r1", "turn_id": "t1", "request_kind": "command"}], "questions": [], "secrets": []});
        for run in [following(None, None), following(Some("t1"), None), following(None, Some("op"))] {
            let door = Door::fake(vec![busy(), ("thread_wait", Ok(waited("needs_input", json!({"pending": approval}))))]);
            let report = follow_run(&door, run.clone(), None, false).await;
            assert_eq!(report.state, RunState::NeedsInput, "{run:?}");
            assert_eq!(door.script().calls()[1].1["since"], "cur-first", "{run:?}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn failed_runs_and_receipts_exit_1() {
        let door = Door::fake(vec![busy(), ("thread_wait", Ok(waited("failed", json!({"failure": "interrupted"}))))]);
        let report = follow(&door, None).await;
        assert_eq!((report.state.exit().code(), report.failure.as_deref()), (1, Some("interrupted")));

        for (state, error) in [
            ("failed", json!({"code": "provider_not_ready", "message": "Starting", "retryable": true})),
            ("needs_attention", Value::Null),
        ] {
            let mut failed = receipt(state, Value::Null);
            failed["error"] = error;
            let report = *followed(Ok(failed), "ws", None, "k").unwrap_err();
            assert_eq!((report.state, report.thread_id.as_deref()), (RunState::Failed, Some("th-1")), "{state}");
            assert!(report.error.is_some(), "{state}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn unknown_wait_state_fails_closed() {
        let door = Door::fake(vec![busy(), ("thread_wait", Ok(waited("paused", json!({}))))]);
        let report = follow(&door, None).await;
        assert_eq!((report.state, report.error.unwrap().code), (RunState::Failed, code::UNEXPECTED_STATE.to_owned()));
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_exits_3_and_never_cancels() {
        let timed_out = || Ok(waited("running", json!({"timed_out": true})));
        let door = Door::fake(vec![busy(), ("thread_wait", timed_out()), ("thread_wait", timed_out())]);
        let report = follow(&door, Some(Instant::now() + Duration::from_secs(90))).await;
        assert_eq!((report.state, report.state.exit().code()), (RunState::TimedOut, 3));
        assert_eq!(report.error.unwrap().code, code::TIMED_OUT);
        // Two slices fit the deadline exactly; nothing else was called.
        assert_eq!(door.script().names(), [FIRST_READ, "thread_wait", "thread_wait"]);
        assert_eq!(door.script().calls()[2].1["timeout_ms"], 30_000);
    }

    #[tokio::test(start_paused = true)]
    async fn the_deadline_bounds_every_call_to_a_stalled_app() {
        let second = Duration::from_secs(1);
        let timed_out = |error: Option<ErrorBody>| error.map(|error| error.code) == Some(code::TIMED_OUT.to_owned());
        // The follow's first read, and a later wait whose reply never comes.
        for script in [vec![stalled(FIRST_READ)], vec![quick(busy()), stalled("thread_wait")]] {
            let door = Door::fake_slow(script);
            let started = Instant::now();
            let report = follow(&door, Some(started + second)).await;
            assert_eq!((report.state, started.elapsed()), (RunState::TimedOut, second));
            assert!(timed_out(report.error), "the deadline, not the call's own bound");
        }

        // A slow first read leaves the first wait only the time that is left.
        let door = Door::fake_slow(vec![
            (busy().0, Duration::from_millis(400), busy().1),
            quick(("thread_wait", Ok(waited("running", json!({"timed_out": true}))))),
        ]);
        let started = Instant::now();
        let report = follow(&door, Some(started + second)).await;
        assert_eq!((report.state, started.elapsed()), (RunState::TimedOut, second));
        assert_eq!(door.script().calls()[1].1["timeout_ms"], 600);

        // The mutation itself, which is the verb's first call.
        let door = Door::fake_slow(vec![stalled("thread_launch")]);
        let started = Instant::now();
        let exit = start(&door, start_args(&[base().as_slice(), &["--json", "--request-id", "k", "--timeout", "1s"]].concat())).await.unwrap();
        assert_eq!((exit, started.elapsed()), (Exit::TimedOut, second));

        // A settle poll keeps the receipt it had when the deadline came.
        let accepted = receipt("accepted", Value::Null);
        let door = Door::fake_slow(vec![quick(("thread_launch", Ok(accepted.clone()))), stalled("operation_status")]);
        let started = Instant::now();
        let failure = mutate(&door, "thread_launch", json!({"client_request_id": "k"}), Some(started + second)).await.unwrap_err();
        assert_eq!((failure.exit, started.elapsed()), (Exit::TimedOut, second));
        assert_eq!((failure.error.code.as_str(), failure.last), (code::TIMED_OUT, Some(accepted)));
        assert_eq!(door.script().names(), ["thread_launch", "operation_status"], "a deadline never cancels work");
    }

    #[tokio::test(start_paused = true)]
    async fn the_deadline_bounds_retries_and_their_backoff() {
        let unavailable = || ("thread_wait", Err(CliError::failed(code::UNAVAILABLE, "restarting")));
        let door = Door::fake(vec![
            busy(),
            ("thread_wait", Ok(waited("running", json!({})))),
            unavailable(),
            unavailable(),
            unavailable(),
            unavailable(),
            unavailable(),
            ("thread_wait", Ok(waited("done", json!({"final_text": "after the deadline"})))),
        ]);
        let started = Instant::now();
        let report = follow(&door, Some(started + Duration::from_secs(2))).await;
        assert_eq!((report.state, started.elapsed()), (RunState::TimedOut, Duration::from_secs(2)));
        // The report keeps what the last wait said.
        assert_eq!((report.turn_id.as_deref(), report.final_text), (Some("t1"), None));
        assert!(!door.script().finished(), "retries stop at the deadline");

        // A settle's polls too.
        let polling = || ("operation_status", Err(CliError::failed(code::UNAVAILABLE, "restarting")));
        let accepted = receipt("accepted", Value::Null);
        let door = Door::fake(vec![
            ("thread_launch", Ok(accepted.clone())),
            polling(),
            polling(),
            polling(),
            polling(),
            ("operation_status", Ok(receipt("succeeded", json!({"turn": {"turn_id": "t1"}})))),
        ]);
        let started = Instant::now();
        let failure = mutate(&door, "thread_launch", json!({"client_request_id": "k"}), Some(started + Duration::from_secs(1))).await.unwrap_err();
        assert_eq!((failure.exit, started.elapsed()), (Exit::TimedOut, Duration::from_secs(1)));
        assert_eq!(failure.last, Some(accepted));
    }

    #[tokio::test(start_paused = true)]
    async fn transient_errors_in_the_loop_are_retried_with_the_same_arguments() {
        let door = Door::fake(vec![
            busy(),
            ("thread_wait", Ok(waited("running", json!({})))),
            ("thread_wait", Err(CliError::failed(code::NO_REPLY, "lost"))),
            ("thread_wait", Err(CliError::failed(code::UNAVAILABLE, "restarting"))),
            ("thread_wait", Ok(waited("done", json!({"final_text": "ok"})))),
        ]);
        let report = follow(&door, None).await;
        assert_eq!(report.state, RunState::Done);
        let calls = door.script().calls();
        assert_eq!(calls[2].1, calls[3].1);
        assert_eq!(calls[3].1, calls[4].1);

        // A mutation's settle polls repeat the identical request too.
        let door = Door::fake(vec![
            ("thread_send", Ok(json!({"operation_id": "op-2", "state": "accepted"}))),
            ("operation_status", Err(CliError::failed(code::UNAVAILABLE, "restarting"))),
            ("operation_status", Ok(json!({"operation_id": "op-2", "state": "succeeded", "thread_id": "th", "result": {"accepted_as": "queued", "turn": {"turn_id": ""}}}))),
        ]);
        let settled = mutate(&door, "thread_send", json!({"client_request_id": "k"}), None).await.unwrap();
        assert_eq!(settled["state"], "succeeded");
        assert_eq!(door.script().calls()[1].1, door.script().calls()[2].1);
    }

    #[tokio::test(start_paused = true)]
    async fn first_call_is_not_retried() {
        let door = Door::fake(vec![(FIRST_READ, Err(CliError::failed(code::UNAVAILABLE, "Start the Codemux app")))]);
        let report = follow(&door, None).await;
        assert_eq!((report.state.exit().code(), report.error.unwrap().code), (1, code::UNAVAILABLE.to_owned()));
        assert!(door.script().finished());

        let door = Door::fake(vec![("thread_launch", Err(CliError::failed(code::UNAVAILABLE, "Start the Codemux app")))]);
        assert_eq!(start(&door, start_args(&[base().as_slice(), &["--wait", "--json"]].concat())).await.unwrap(), Exit::Failed);
        assert_eq!(door.script().names(), ["thread_launch"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_queued_send_follows_its_operation_to_its_own_run() {
        // The queued receipt names only the queue entry, so the follow waits
        // on the operation: the server reads `running` until the message's
        // run starts, then follows that run.
        let receipt = |result: Value| json!({"operation_id": "op", "tool": "thread_send", "state": "succeeded", "workspace_id": "ws",
            "thread_id": "th", "result": result, "error": null});
        let queued = receipt(json!({"workspace_id": "ws", "thread_id": "th", "accepted_as": "queued", "turn": {"queued_id": "q1"}}));
        let run = followed(Ok(queued), "ws", Some("th"), "k").unwrap();
        assert_eq!((run.turn_id.as_deref(), run.operation_id.as_deref()), (None, Some("op")));
        let operation = |state: &str, error: Value| json!({"operation_id": "op", "state": state, "error": error});
        let not_started = json!({"turn_id": null, "operation": operation("succeeded", Value::Null)});
        let door = Door::fake(vec![
            ("thread_wait", Ok(waited("running", not_started.clone()))),
            ("thread_wait", Ok(waited("running", json!({"turn_id": "t2", "operation": operation("succeeded", Value::Null)})))),
            ("thread_wait", Ok(waited("done", json!({"turn_id": "t2", "final_text": "queued run", "operation": operation("succeeded", Value::Null)})))),
        ]);
        let report = follow_run(&door, run.clone(), None, true).await;
        assert_eq!((report.state, report.turn_id.as_deref(), report.final_text.as_deref()), (RunState::Done, Some("t2"), Some("queued run")));
        let calls = door.script().calls();
        assert_eq!(calls[0].1, json!({"workspace_id": "ws", "thread_id": "th", "operation_id": "op", "timeout_ms": 1}));
        assert_eq!((calls[1].1["operation_id"].as_str(), calls[1].1["since"].as_str()), (Some("op"), Some("cur-running")));
        assert!(calls.iter().all(|(_, arguments)| arguments.get("turn_id").is_none()), "the server picks the run");
        assert!(door.script().finished());

        // A message that never runs fails with its receipt's own error and
        // none of another run's answer: cancelled from the queue, or held by
        // a stop.
        for (state, code) in [("failed", "cancelled"), ("needs_attention", "delivery_held")] {
            let ended = json!({"turn_id": null, "failure": "operation_failed", "final_text": null,
                "operation": operation(state, json!({"code": code, "message": "It never ran"}))});
            let door = Door::fake(vec![
                ("thread_wait", Ok(waited("running", not_started.clone()))),
                ("thread_wait", Ok(waited("failed", ended))),
            ]);
            let report = follow_run(&door, run.clone(), None, true).await;
            assert_eq!((report.state, report.error.map(|error| error.code)), (RunState::Failed, Some(code.to_owned())), "{state}");
            assert_eq!((report.turn_id, report.final_text, report.failure), (None, None, None), "{state}");
        }

        // Input that holds the queue is reported, as for any follow.
        let approval = json!({"approvals": [{"request_id": "r1", "turn_id": "t1", "request_kind": "command"}], "questions": [], "secrets": []});
        let door = Door::fake(vec![
            ("thread_wait", Ok(waited("needs_input", json!({"turn_id": null, "pending": approval, "operation": operation("succeeded", Value::Null)})))),
        ]);
        let report = follow_run(&door, run, None, true).await;
        assert_eq!((report.state, report.turn_id, report.pending), (RunState::NeedsInput, None, Some(approval)));
    }

    #[tokio::test(start_paused = true)]
    async fn queued_work_keeps_a_thread_wait_going_but_not_a_runs() {
        // Without a turn, the wait follows the thread until its queue drains.
        let door = Door::fake(vec![
            busy(),
            ("thread_wait", Ok(waited("done", json!({"final_text": "older run", "status": {"queued_ids": ["q1", "q2"], "uncertain_queued_ids": []}})))),
            ("thread_wait", Ok(waited("done", json!({"turn_id": "t3", "final_text": "last run"})))),
        ]);
        let report = follow(&door, None).await;
        assert_eq!((report.state, report.final_text.as_deref()), (RunState::Done, Some("last run")));
        let calls = door.script().calls();
        assert!(calls[2].1.get("turn_id").is_none() && calls[2].1["since"] == "cur-done", "{}", calls[2].1);

        // A named run, or an operation's, ends the follow when it ends; the
        // queue holds other messages.
        for run in [following(Some("t1"), None), following(None, Some("op"))] {
            let door = Door::fake(vec![
                busy(),
                ("thread_wait", Ok(waited("done", json!({"final_text": "t1 text", "status": {"queued_ids": ["q5"], "uncertain_queued_ids": []}})))),
            ]);
            let report = follow_run(&door, run, None, true).await;
            assert_eq!((report.state, report.final_text.as_deref()), (RunState::Done, Some("t1 text")));
        }

        // A delivery that needs the person ends the wait instead.
        let stuck = json!({"failure": "delivery_needs_attention", "status": {"queued_ids": [], "uncertain_queued_ids": ["q9"]}});
        let door = Door::fake(vec![("thread_wait", Ok(waited("failed", stuck)))]);
        let report = follow_run(&door, following(None, None), None, false).await;
        assert_eq!((report.state, report.failure.as_deref()), (RunState::Failed, Some("delivery_needs_attention")));
    }

    #[tokio::test(start_paused = true)]
    async fn status_exits_0_whatever_the_run_state() {
        for state in ["running", "needs_input", "done", "failed"] {
            for json in [true, false] {
                let door = Door::fake(vec![("thread_status", Ok(status_reply(state, json!({"phase": "completed", "settled": true}))))]);
                let args = StatusArgs { thread: "th".into(), workspace: Some("ws".into()), out: OutputArgs { json } };
                assert_eq!(status(&door, args).await.unwrap(), Exit::Done, "{state} json={json}");
            }
        }
    }

    #[test]
    fn wait_slice_fits_the_thread_wait_schema() {
        let listed = crate::agent_control::registry::list(&crate::agent_control::CallerView::outside(crate::agent_control::ControlAccess::FullAccess));
        let wait = listed.iter().find(|tool| tool.definition.name == verbs::THREAD_WAIT).expect("thread_wait is an outside tool");
        let maximum = wait.definition.input_schema["properties"]["timeout_ms"]["maximum"].as_u64().unwrap();
        assert!(WAIT_SLICE.as_millis() as u64 <= maximum, "{WAIT_SLICE:?} > {maximum} ms");
    }

    #[test]
    fn run_report_json_is_stable() {
        let run = Followed { client_request_id: Some("k".into()), ..following(None, None) };
        let report = serde_json::to_value(RunReport::of(&run).finish(RunState::NeedsInput)).unwrap();
        let mut keys: Vec<&str> = report.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["client_request_id", "error", "failure", "final_text", "ok", "operation_id", "pending", "state", "thread_id", "turn_id", "workspace_id"]);
        assert_eq!(report["state"], "needs_input");
        let printed = serde_json::to_string(&RunReport::of(&run)).unwrap();
        assert!(printed.starts_with(r#"{"ok":false,"state":"failed","failure":null,"workspace_id":"ws","thread_id":"th""#), "{printed}");
        for (state, name) in [(RunState::Done, "done"), (RunState::Failed, "failed"), (RunState::TimedOut, "timed_out")] {
            assert_eq!(serde_json::to_value(state).unwrap(), name);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn send_reports_how_the_message_was_taken() {
        let door = Door::fake(vec![
            ("thread_send", Ok(json!({"operation_id": "op", "state": "succeeded", "result": {"accepted_as": "steered", "turn": {"turn_id": "t1"}}}))),
        ]);
        let send_args = match Cli::try_parse_from(["codemux", "thread", "send", "th", "--workspace", "ws", "-m", "more", "--delivery", "steer", "--request-id", "k"]).unwrap().command {
            Some(CommandSet::Thread { command: ThreadCommand::Send(args) }) => args,
            _ => panic!("expected thread send"),
        };
        assert_eq!(send(&door, send_args).await.unwrap(), Exit::Done);
        assert_eq!(
            door.script().calls()[0].1,
            json!({"workspace_id": "ws", "thread_id": "th", "client_request_id": "k", "message": "more", "delivery": "steer"})
        );
    }

    #[tokio::test(start_paused = true)]
    async fn list_and_read_merge_every_page() {
        let door = Door::fake(vec![
            ("thread_list", Ok(json!({"threads": [{"thread_id": "a"}, {"thread_id": "b"}], "has_more": true, "next_cursor": "b"}))),
            ("thread_list", Ok(json!({"threads": [{"thread_id": "c"}], "has_more": false, "next_cursor": null}))),
        ]);
        let list_args = ListArgs { workspace: Some("ws".into()), limit: None, out: OutputArgs { json: true } };
        assert_eq!(list(&door, list_args).await.unwrap(), Exit::Done);
        assert_eq!(door.script().calls()[1].1, json!({"workspace_id": "ws", "limit": 100, "cursor": "b"}));

        let door = Door::fake(vec![("thread_list", Ok(json!({"threads": [{"thread_id": "a"}, {"thread_id": "b"}], "has_more": false, "next_cursor": null})))]);
        let list_args = ListArgs { workspace: Some("ws".into()), limit: Some(1), out: OutputArgs { json: true } };
        assert_eq!(list(&door, list_args).await.unwrap(), Exit::Done);

        let door = Door::fake(vec![
            ("thread_read", Ok(json!({"messages": [{"message_id": 1}], "next_cursor": 1}))),
            ("thread_read", Ok(json!({"messages": [{"message_id": 2}], "next_cursor": null}))),
        ]);
        let read_args = ReadArgs { thread: "th".into(), workspace: Some("ws".into()), cursor: None, limit: 1, all: true, out: OutputArgs { json: true } };
        assert_eq!(read(&door, read_args).await.unwrap(), Exit::Done);
        assert_eq!(door.script().calls()[1].1, json!({"workspace_id": "ws", "thread_id": "th", "limit": 1, "cursor": 1}));
    }

    #[tokio::test(start_paused = true)]
    async fn respond_sends_the_decision_and_its_own_key() {
        let door = Door::fake(vec![("thread_respond", Ok(json!({"operation_id": "op", "state": "succeeded"})))]);
        let respond_args = match Cli::try_parse_from(["codemux", "thread", "respond", "th", "r1", "allow-once", "--workspace", "ws", "--request-key", "k"]).unwrap().command {
            Some(CommandSet::Thread { command: ThreadCommand::Respond(args) }) => args,
            _ => panic!("expected thread respond"),
        };
        assert_eq!(respond(&door, respond_args).await.unwrap(), Exit::Done);
        assert_eq!(
            door.script().calls()[0].1,
            json!({"workspace_id": "ws", "thread_id": "th", "request_id": "r1", "decision": "allow_once", "client_request_id": "k"})
        );
    }

    fn answer_args(extra: &[&str]) -> AnswerArgs {
        let argv = [&["codemux", "thread", "answer", "th", "--workspace", "ws", "--question", "q1"][..], extra].concat();
        match Cli::try_parse_from(argv).unwrap().command {
            Some(CommandSet::Thread { command: ThreadCommand::Answer(args) }) => args,
            _ => panic!("expected thread answer"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn answer_sends_every_answer_in_order_under_its_key() {
        let answered = json!({"workspace_id": "ws", "thread_id": "th", "question_id": "q1", "turn_id": "t2", "operation_id": "op", "state": "succeeded"});
        let door = Door::fake(vec![("thread_answer", Ok(answered))]);
        assert_eq!(answer(&door, answer_args(&["--answer", "staging", "--answer", "yes", "--request-id", "k", "--json"])).await.unwrap(), Exit::Done);
        assert_eq!(
            door.script().calls()[0].1,
            json!({"workspace_id": "ws", "thread_id": "th", "client_request_id": "k", "question_id": "q1", "answers": ["staging", "yes"]})
        );

        // A retry while the first call runs settles its receipt.
        let door = Door::fake(vec![
            ("thread_answer", Ok(json!({"operation_id": "op", "state": "running"}))),
            ("operation_status", Ok(json!({"operation_id": "op", "state": "succeeded", "result": {"turn_id": "t2"}}))),
        ]);
        assert_eq!(answer(&door, answer_args(&["--answer", "staging", "--request-id", "k"])).await.unwrap(), Exit::Done);
        assert!(door.script().finished());

        // The tool needs an answer, so the CLI refuses none before calling.
        let none = Cli::try_parse_from(["codemux", "thread", "answer", "th", "--workspace", "ws", "--question", "q1"]);
        assert!(none.is_err());
        // The thread is positional, as in every other thread verb.
        let flagged = Cli::try_parse_from(["codemux", "thread", "answer", "--thread", "th", "--question", "q1", "--answer", "a"]);
        assert!(flagged.is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_answer_exits_1_also_when_its_retry_returns_the_receipt() {
        let door = Door::fake(vec![("thread_answer", Err(CliError::failed("question_closed", "This question was already answered or dismissed")))]);
        assert_eq!(answer(&door, answer_args(&["--answer", "staging"])).await.unwrap(), Exit::Failed);

        // A keyed retry of an answer that failed gets its receipt as a result.
        let failed = json!({"operation_id": "op", "tool": "thread_answer", "state": "failed", "result": null,
            "error": {"code": "question_answer_failed", "message": "The worker did not take the answer", "retryable": false}});
        let door = Door::fake(vec![("thread_answer", Ok(failed))]);
        assert_eq!(answer(&door, answer_args(&["--answer", "staging", "--request-id", "k", "--json"])).await.unwrap(), Exit::Failed);
    }
}
