//! Watch, unwatch and read a chat's pull-request watch (`crate::pr_watch`),
//! from the UI (Tauri commands) and from the chat's own agent (`pr_watch`
//! host tool).
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Runtime};

use crate::agent_provider::ProviderKind;
use crate::database::pr_watch::{AgentWatchStart, PrWatchRecord, PR_WATCH_ACTIVE};
use crate::database::DatabaseStore;
use crate::mcp::registry::{HostCaller, HostTools};
use crate::mcp::runtime::{prefix_tool_name, McpTool};
use crate::pr_watch::{DEFAULT_MAX_ATTEMPTS, MAX_ATTEMPTS_LIMIT};
use crate::state::AppStateStore;

/// Start watching a pull request from a chat, or re-arm the chat's watch.
/// `pr_number` defaults to the pull request linked to the chat's workspace.
#[tauri::command]
pub async fn pr_watch_start<R: Runtime>(
    app: AppHandle<R>,
    thread_id: String,
    pr_number: Option<u32>,
    max_attempts: Option<u32>,
) -> Result<PrWatchRecord, String> {
    let max_attempts = max_attempts.unwrap_or(DEFAULT_MAX_ATTEMPTS);
    if !(1..=MAX_ATTEMPTS_LIMIT).contains(&max_attempts) {
        return Err(format!("max_attempts must be between 1 and {MAX_ATTEMPTS_LIMIT}"));
    }
    let db = app.state::<DatabaseStore>();
    let session = db
        .get_agent_chat_session(&thread_id)
        .ok_or("No chat has this thread ID")?;
    if session.imported_from.is_some() {
        return Err("Imported chats are read-only snapshots".into());
    }
    let workspace = app
        .state::<AppStateStore>()
        .find_workspace(&session.workspace_id)
        .ok_or("The chat's workspace is not open")?;
    if workspace.host_id.is_some() || workspace.remote_cwd.is_some() {
        return Err("Pull request watching runs for local workspaces only".into());
    }
    let pr_number = pr_number
        .or(workspace.pr_number)
        .ok_or("This workspace has no linked pull request")?;
    let known_url = workspace
        .pr_url
        .clone()
        .filter(|_| workspace.pr_number == Some(pr_number));

    // The watch is pinned to the PR's canonical URL: every sweep reads that
    // repository, whatever the checkout points at later.
    let repo = std::path::PathBuf::from(&workspace.cwd);
    let pr_url = tokio::task::spawn_blocking(move || -> Result<String, String> {
        let detected = crate::git_provider::try_detect_provider(&repo)?;
        if detected.kind != crate::git_provider::ProviderKind::GitHub {
            return Err("Pull request watching supports GitHub repositories only".into());
        }
        if let Some(url) = known_url {
            return Ok(url);
        }
        let number = pr_number.to_string();
        let json = crate::github_budget::run(
            &repo,
            &["pr", "view", &number, "--json", "url"],
            std::time::Duration::from_secs(20),
        )?;
        serde_json::from_str::<serde_json::Value>(&json)
            .ok()
            .and_then(|v| v["url"].as_str().map(str::to_string))
            .ok_or_else(|| format!("Could not find pull request #{pr_number}"))
    })
    .await
    .map_err(|e| format!("Pull request lookup failed: {e}"))??;
    if crate::pr_watch::parse_pr_url(&pr_url, pr_number).is_none() {
        return Err(format!("{pr_url} is not the address of pull request #{pr_number}"));
    }

    let record = db.arm_pr_watch(
        &thread_id,
        &session.workspace_id,
        pr_number,
        Some(&pr_url),
        max_attempts,
        crate::pr_watch::now_ms(),
    )?;
    crate::pr_watch::emit_changed(&app, &thread_id, Some(record.clone()));
    crate::pr_watch::kick();
    Ok(record)
}

/// Stop watching and forget what the chat was told. Returns whether a watch
/// existed.
#[tauri::command]
pub async fn pr_watch_stop<R: Runtime>(app: AppHandle<R>, thread_id: String) -> Result<bool, String> {
    let removed = app.state::<DatabaseStore>().delete_pr_watch(&thread_id)?;
    if removed {
        crate::pr_watch::emit_changed(&app, &thread_id, None);
    }
    Ok(removed)
}

/// The chat's watch, including a stopped one and why it stopped.
#[tauri::command]
pub async fn pr_watch_status<R: Runtime>(
    app: AppHandle<R>,
    thread_id: String,
) -> Result<Option<PrWatchRecord>, String> {
    app.state::<DatabaseStore>().get_pr_watch(&thread_id)
}

// ── Agent-facing tool ──────────────────────────────────────────────────

const TOOL_NAME: &str = "pr_watch";

/// `pr_watch` as host tools are listed to agent chats.
pub const PR_WATCH_REGISTRY_NAME: &str = "mcp__codemux__pr_watch";

/// Session instructions for chats that list `pr_watch`. Claude sees MCP tools
/// by name only until it searches for one, so without this an agent babysits
/// a pull request by polling `gh` in the shell.
pub const PR_WATCH_GUIDANCE: &str = "## Watching pull requests in Codemux\n\n\
When the user asks you to monitor, watch or babysit a pull request, or to wait for its CI, call Codemux's `pr_watch` tool \
(listed with an MCP prefix) with action `start`, handle the checks and comments that already exist, then end your turn. \
Do not poll `gh pr checks`, `gh run watch` or the GitHub API in a loop, and do not sleep waiting for CI: Codemux checks the \
pull request about every two minutes and wakes this chat with a new message when a check fails, someone else comments or reviews, \
the branch conflicts with its base, or every check passes on the latest commit and it merges cleanly. Use action `status` only when you need the last \
result mid-turn. Call action `stop` when the user no longer wants the pull request watched.";

const TOOL_DESCRIPTION: &str = "Have Codemux watch this chat's GitHub pull request and wake this chat with a new message when a check fails, someone else comments or reviews, the branch starts to conflict with its base, or every check passes and it merges cleanly (once per pushed commit). Use it instead of polling `gh` or sleeping: after `start`, handle what is already there and END YOUR TURN.

Actions: `start` watches the pull request linked to this workspace (or `pr_number`, which must be one of the workspace's linked pull requests); repeating it changes nothing, and after `stop` it continues where the watch stopped. `status` reads the watch, including why it stopped. `stop` stops watching. Automatic fix wakes are capped (3 when started here); once they run out, or to move the watch to another open pull request, only the user can re-arm it in Codemux. Local GitHub workspaces only; a wake can be a few minutes late.";

/// How long a workspace's pull-request refresh, asked for by `start` on a
/// pull request Codemux has not linked yet, holds off another one.
const LINK_REFRESH_COOLDOWN: Duration = Duration::from_secs(60);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolArgs {
    action: ToolAction,
    pr_number: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ToolAction {
    Start,
    Status,
    Stop,
}

impl ToolArgs {
    fn parse(arguments: &Value) -> Result<Self, String> {
        let args: Self = serde_json::from_value(arguments.clone()).map_err(|error| {
            format!("Pass action (start, status or stop) and, for start, an optional pr_number: {error}")
        })?;
        if args.pr_number.is_some() && args.action != ToolAction::Start {
            return Err("pr_number applies to start only.".into());
        }
        Ok(args)
    }
}

/// Serves `pr_watch` to Claude and Codex chats. The watch always belongs to
/// the calling chat ([`HostCaller`]), never to a thread the model names.
pub struct PrWatchHost<R: Runtime> {
    app: AppHandle<R>,
    /// Ask Codemux to look up the workspace's pull requests again, without
    /// waiting. Replaced in tests, which have no `gh`.
    refresh_links: fn(&AppHandle<R>, String),
    /// Whether the checkout's remote is GitHub. Replaced in tests, since
    /// detection reads the user's synced custom hosts.
    is_github: fn(&std::path::Path) -> Result<bool, String>,
}

impl<R: Runtime> PrWatchHost<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self { app, refresh_links: spawn_link_refresh, is_github: detect_github }
    }
}

fn detect_github(repo: &std::path::Path) -> Result<bool, String> {
    Ok(crate::git_provider::try_detect_provider(repo)?.kind == crate::git_provider::ProviderKind::GitHub)
}

fn spawn_link_refresh<R: Runtime>(app: &AppHandle<R>, workspace_id: String) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppStateStore>();
        if let Err(error) = super::github::refresh_workspace_pr(app.clone(), state, workspace_id).await {
            eprintln!("[codemux::pr-watch] link refresh failed: {error}");
        }
    });
}

/// Whether a link refresh for `workspace_id` may start now; claims it if so.
fn claim_link_refresh(workspace_id: &str, now: Instant) -> bool {
    static LAST: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    let mut last = LAST
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    last.retain(|_, at| now.duration_since(*at) < LINK_REFRESH_COOLDOWN);
    if last.contains_key(workspace_id) {
        return false;
    }
    last.insert(workspace_id.to_string(), now);
    true
}

/// A chat that cannot change files cannot act on a wake, so it may not
/// start one. Stopping and reading stay available. A Codex Plan turn calls
/// as `plan` too (`CodexSession::host_caller_for_call`).
fn can_start(caller: &HostCaller) -> bool {
    !matches!(caller.permission_mode.as_deref(), Some("plan" | "read-only"))
}

fn tool() -> McpTool {
    McpTool {
        name: TOOL_NAME.into(),
        prefixed_name: prefix_tool_name("codemux", TOOL_NAME),
        server_id: "codemux-host".into(),
        description: Some(TOOL_DESCRIPTION.into()),
        input_schema: json!({
            "type": "object", "additionalProperties": false,
            "properties": {
                "action": {"type": "string", "enum": ["start", "status", "stop"]},
                "pr_number": {"type": "integer", "minimum": 1, "description": "start only: a pull request linked to this workspace. Omit for the workspace's pull request."}
            },
            "required": ["action"]
        }),
    }
}

/// What the agent reads about a watch.
fn watch_view(watch: &PrWatchRecord) -> Value {
    json!({
        "pr_number": watch.pr_number,
        "pr_url": watch.pr_url,
        "state": watch.status,
        "fix_attempts_used": watch.attempts,
        "max_fix_attempts": watch.max_attempts,
        "last_checked_at_ms": watch.last_checked_at_ms,
        "last_wake_at_ms": watch.last_wake_at_ms,
        "last_wake_summary": watch.last_wake_summary,
        "last_error": watch.last_error,
        "stop_reason": watch.stop_reason,
    })
}

const STOPPED_BY_AGENT: &str = "Stopped by the agent";

const END_TURN: &str = "Codemux checks the pull request about every two minutes and wakes this chat with a message on a failed check, a new comment or review from someone else, a merge conflict, or once all checks pass on the latest commit and it merges cleanly. Handle what is already there, then end your turn; do not poll.";

impl<R: Runtime> PrWatchHost<R> {
    fn answer(&self, caller: &HostCaller, arguments: &Value) -> Result<Value, String> {
        let args = ToolArgs::parse(arguments)?;
        let db = self.app.try_state::<DatabaseStore>().ok_or("Pull request watching is unavailable.")?;
        let thread_id = caller.thread_id.as_str();
        match args.action {
            ToolAction::Status => Ok(match db.get_pr_watch(thread_id)? {
                Some(watch) if watch.status == PR_WATCH_ACTIVE => json!({"status": "watching", "watch": watch_view(&watch)}),
                Some(watch) => json!({"status": "not_watching", "watch": watch_view(&watch)}),
                None => json!({"status": "not_watching"}),
            }),
            // The record stays (unlike the user's unwatch), so stopping and
            // starting again cannot reset the fix attempts or replay events.
            // A wake between its commit and its delivery may still hand its
            // events back; stopping then would fence that out and lose them.
            ToolAction::Stop => match crate::pr_watch::unless_waking(thread_id, || {
                db.end_pr_watch_for_agent(thread_id, STOPPED_BY_AGENT, crate::pr_watch::now_ms())
            })
            .ok_or("Codemux is delivering a pull request update to this chat right now; it arrives as your next message. Call stop again after it.")??
            {
                Some(watch) => {
                    crate::pr_watch::emit_changed(&self.app, thread_id, Some(watch.clone()));
                    Ok(json!({"status": "stopped", "watch": watch_view(&watch)}))
                }
                None => Ok(match db.get_pr_watch(thread_id)? {
                    Some(watch) => json!({"status": "not_watching", "watch": watch_view(&watch)}),
                    None => json!({"status": "not_watching"}),
                }),
            },
            ToolAction::Start => self.start(caller, &db, args.pr_number),
        }
    }

    fn start(&self, caller: &HostCaller, db: &DatabaseStore, pr_number: Option<u32>) -> Result<Value, String> {
        if !can_start(caller) {
            return Err("This chat cannot change files in its current mode, so it cannot act on pull request updates. Ask the user to switch modes or to watch it from Codemux.".into());
        }
        let thread_id = caller.thread_id.as_str();
        let session = db.get_agent_chat_session(thread_id).ok_or("This chat has no saved session yet.")?;
        if session.imported_from.is_some() {
            return Err("Imported chats are read-only snapshots".into());
        }
        let workspace = self
            .app
            .state::<AppStateStore>()
            .find_workspace(&session.workspace_id)
            .ok_or("The chat's workspace is not open")?;
        if workspace.host_id.is_some() || workspace.remote_cwd.is_some() {
            return Err("Pull request watching runs for local workspaces only".into());
        }
        // Only a pull request Codemux itself linked to the workspace: its
        // address comes from GitHub, not from the model.
        let wanted = pr_number.or(workspace.pr_number);
        let linked = wanted.and_then(|number| {
            workspace
                .prs
                .iter()
                .find(|pr| pr.number == number)
                .map(|pr| pr.url.clone())
                .or_else(|| workspace.pr_url.clone().filter(|_| workspace.pr_number == Some(number)))
                .map(|url| (number, url))
        });
        let Some((pr_number, pr_url)) = linked else {
            let what = wanted.map_or("No pull request is".to_string(), |n| format!("Pull request #{n} is not"));
            if claim_link_refresh(&workspace.workspace_id.0, Instant::now()) {
                (self.refresh_links)(&self.app, workspace.workspace_id.0.clone());
            }
            return Err(format!(
                "{what} linked to this workspace yet. Codemux is looking up the workspace's pull requests now; call start once more in a later step. If it is still not linked, tell the user they can watch it from the chat in Codemux, and do not poll."
            ));
        };
        if crate::pr_watch::parse_pr_url(&pr_url, pr_number).is_none() {
            return Err("Pull request watching supports GitHub pull requests only".into());
        }
        if !(self.is_github)(std::path::Path::new(&workspace.cwd))? {
            return Err("Pull request watching supports GitHub repositories only".into());
        }

        let outcome = db.start_pr_watch_for_agent(
            thread_id,
            &session.workspace_id,
            pr_number,
            &pr_url,
            crate::pr_watch::now_ms(),
        )?;
        match outcome {
            AgentWatchStart::Started(record) => {
                crate::pr_watch::emit_changed(&self.app, thread_id, Some(record.clone()));
                crate::pr_watch::kick();
                Ok(json!({"status": "watching", "watch": watch_view(&record), "note": END_TURN}))
            }
            AgentWatchStart::AlreadyWatching(record) => {
                Ok(json!({"status": "already_watching", "watch": watch_view(&record), "note": END_TURN}))
            }
            AgentWatchStart::Exhausted(record) => Err(format!(
                "Watching #{} stopped because its automatic fix attempts ran out. Only the user can re-arm it in Codemux; tell them what is still failing.",
                record.pr_number
            )),
            AgentWatchStart::OtherPullRequest(record) => Err(format!(
                "This chat's watch is on pull request #{}, which is still open. Only the user can move it to another pull request in Codemux.",
                record.pr_number
            )),
        }
    }
}

impl<R: Runtime> HostTools for PrWatchHost<R> {
    fn tools_for(&self, caller: &HostCaller) -> Vec<McpTool> {
        // A delegated task reports to its parent, which owns the pull request.
        let is_child = self
            .app
            .try_state::<super::delegation::DelegationState>()
            .is_some_and(|state| state.is_running_child(&caller.thread_id));
        if !matches!(caller.provider, ProviderKind::Claude | ProviderKind::Codex) || is_child {
            return vec![];
        }
        vec![tool()]
    }

    fn call(&self, caller: &HostCaller, name: &str, arguments: &Value) -> Option<Value> {
        if name != PR_WATCH_REGISTRY_NAME {
            return None;
        }
        let (text, is_error) = if self.tools_for(caller).is_empty() {
            ("pr_watch is not available to this chat.".to_string(), true)
        } else {
            match self.answer(caller, arguments) {
                Ok(result) => (result.to_string(), false),
                Err(error) => (error, true),
            }
        };
        Some(json!({"content": [{"type": "text", "text": text}], "isError": is_error}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::state::WorkspacePr;

    static REFRESHES: AtomicUsize = AtomicUsize::new(0);

    fn count_refresh<R: Runtime>(_: &AppHandle<R>, _: String) {
        REFRESHES.fetch_add(1, Ordering::SeqCst);
    }

    fn caller(provider: ProviderKind, mode: Option<&str>) -> HostCaller {
        HostCaller {
            control: None,
            thread_id: "chat".into(),
            provider,
            workspace_id: None,
            permission_mode: mode.map(str::to_string),
        }
    }

    fn pr(number: u32) -> WorkspacePr {
        WorkspacePr {
            number,
            state: "OPEN".into(),
            url: format!("https://github.com/acme/app/pull/{number}"),
            head_branch: Some("feature".into()),
            base_branch: Some("main".into()),
            source: None,
            checkout_branch: None,
        }
    }

    /// A Codemux-like app with one chat in a local checkout whose workspace
    /// is linked to pull request #7.
    fn app() -> (tauri::App<tauri::test::MockRuntime>, tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_app();
        app.manage(DatabaseStore::new_in_memory());
        app.manage(AppStateStore::default());
        app.manage(super::super::delegation::DelegationState::default());
        let state = app.state::<AppStateStore>();
        let workspace = state.create_empty_workspace_at_path(dir.path().into()).0;
        state.update_workspace_prs(&workspace, vec![pr(7)]);
        app.state::<DatabaseStore>()
            .upsert_agent_chat_session("chat", &workspace, None, "claude")
            .unwrap();
        (app, dir, workspace)
    }

    fn host(app: &tauri::App<tauri::test::MockRuntime>) -> PrWatchHost<tauri::test::MockRuntime> {
        PrWatchHost { app: app.handle().clone(), refresh_links: count_refresh, is_github: |_| Ok(true) }
    }

    fn call(host: &PrWatchHost<tauri::test::MockRuntime>, caller: &HostCaller, args: Value) -> (bool, Value) {
        let answer = host.call(caller, PR_WATCH_REGISTRY_NAME, &args).unwrap();
        let text = answer["content"][0]["text"].as_str().unwrap().to_string();
        let error = answer["isError"].as_bool().unwrap();
        (error, serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    #[test]
    fn pr_watch_tool_listing_brings_its_session_guidance() {
        assert_eq!(tool().prefixed_name, PR_WATCH_REGISTRY_NAME);
        let guidance = crate::mcp_server::tool_guidance([PR_WATCH_REGISTRY_NAME]).unwrap();
        assert!(guidance.contains("`pr_watch`") && guidance.contains("do not sleep"));
        assert!(crate::mcp_server::tool_guidance(["mcp__other__pr_watch"]).is_none());
    }

    #[test]
    fn pr_watch_tool_arguments_cannot_name_another_thread() {
        assert!(ToolArgs::parse(&json!({"action": "start", "pr_number": 7})).is_ok());
        for bad in [
            json!({}),
            json!({"action": "watch"}),
            json!({"action": "status", "pr_number": 7}),
            json!({"action": "start", "thread_id": "someone-else"}),
            json!({"action": "start", "workspace_id": "ws"}),
            json!({"action": "start", "pr_url": "https://github.com/evil/repo/pull/1"}),
            json!({"action": "start", "max_attempts": 10}),
        ] {
            assert!(ToolArgs::parse(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn pr_watch_tool_is_offered_to_main_claude_and_codex_chats_only() {
        let (app, _dir, _) = app();
        let host = host(&app);
        assert_eq!(host.tools_for(&caller(ProviderKind::Claude, Some("plan"))).len(), 1);
        assert_eq!(host.tools_for(&caller(ProviderKind::Codex, Some("read-only"))).len(), 1);
        assert!(host.tools_for(&caller(ProviderKind::Cursor, None)).is_empty());
        assert!(host.tools_for(&caller(ProviderKind::OpenCode, None)).is_empty());
        assert!(host.call(&caller(ProviderKind::Claude, None), "mcp__codemux__other", &json!({})).is_none());

        // A running delegated task is neither offered nor served the tool.
        app.state::<super::super::delegation::DelegationState>().reserve_worktree(
            "chat",
            super::super::delegation::Task::new("parent", ProviderKind::Codex, "t", "b", 0),
        );
        let claude = caller(ProviderKind::Claude, None);
        assert!(host.tools_for(&claude).is_empty());
        let (error, _) = call(&host, &claude, json!({"action": "start"}));
        assert!(error);
        assert!(app.state::<DatabaseStore>().get_pr_watch("chat").unwrap().is_none());
    }

    #[test]
    fn pr_watch_tool_retries_and_stop_then_start_keep_the_attempt_budget() {
        let (app, _dir, workspace) = app();
        let host = host(&app);
        let db = app.state::<DatabaseStore>();
        let claude = caller(ProviderKind::Claude, Some("default"));

        let (error, status) = call(&host, &claude, json!({"action": "status"}));
        assert!(!error);
        assert_eq!(status["status"], "not_watching");

        // Plan mode cannot act on a wake, so it cannot start one.
        let (error, _) = call(&host, &caller(ProviderKind::Claude, Some("plan")), json!({"action": "start"}));
        assert!(error);
        assert!(db.get_pr_watch("chat").unwrap().is_none());

        let (error, started) = call(&host, &claude, json!({"action": "start"}));
        assert!(!error, "{started}");
        assert_eq!(started["status"], "watching");
        assert_eq!(started["watch"]["pr_url"], "https://github.com/acme/app/pull/7");
        assert!(started["note"].as_str().unwrap().contains("end your turn"));
        let armed = db.get_pr_watch("chat").unwrap().unwrap();
        assert_eq!((armed.workspace_id.as_str(), armed.max_attempts), (workspace.as_str(), DEFAULT_MAX_ATTEMPTS));
        assert!(armed.wake_on_green);

        // A wake spent attempts and told an event; a retried start resets
        // neither and keeps the generation a sweep in flight holds.
        let mut spent = armed.clone();
        spent.attempts = 2;
        assert!(db.save_pr_watch_progress(&spent, &["check:a".into()], 1).unwrap());
        let (error, again) = call(&host, &claude, json!({"action": "start", "pr_number": 7}));
        assert!(!error);
        assert_eq!(again["status"], "already_watching");
        let kept = db.get_pr_watch("chat").unwrap().unwrap();
        assert_eq!((kept.attempts, kept.generation), (2, armed.generation));

        // Stop keeps the record and fences the sweep that read the watch.
        let (error, stopped) = call(&host, &claude, json!({"action": "stop"}));
        assert_eq!((error, stopped["status"].as_str()), (false, Some("stopped")));
        let ended = db.get_pr_watch("chat").unwrap().unwrap();
        assert_eq!((ended.status.as_str(), ended.attempts), ("ended", 2));
        assert_eq!(ended.stop_reason.as_deref(), Some(STOPPED_BY_AGENT));
        assert!(!db.save_pr_watch_progress(&kept, &["check:b".into()], 2).unwrap());
        assert!(db.active_pr_watches().unwrap().is_empty());
        let (error, stopped) = call(&host, &claude, json!({"action": "stop"}));
        assert_eq!((error, stopped["status"].as_str()), (false, Some("not_watching")));

        // Starting again continues where it stopped.
        let (error, resumed) = call(&host, &claude, json!({"action": "start"}));
        assert_eq!((error, resumed["status"].as_str()), (false, Some("watching")));
        let resumed = db.get_pr_watch("chat").unwrap().unwrap();
        assert_eq!((resumed.status.as_str(), resumed.attempts), ("active", 2));
        let (_, status) = call(&host, &claude, json!({"action": "status"}));
        assert_eq!((status["status"].as_str(), status["watch"]["fix_attempts_used"].as_u64()), (Some("watching"), Some(2)));
        assert_ne!(resumed.generation, ended.generation);
        assert!(db.pr_watch_seen_keys("chat").unwrap().contains("check:a"));
        assert!(!db.pr_watch_seen_keys("chat").unwrap().contains("check:b"));

        // Moving to another open pull request is the user's call, stopped
        // or not.
        app.state::<AppStateStore>().update_workspace_prs(&workspace, vec![pr(7), pr(8)]);
        let (error, _) = call(&host, &claude, json!({"action": "start", "pr_number": 8}));
        assert!(error);
        call(&host, &claude, json!({"action": "stop"}));
        let (error, text) = call(&host, &claude, json!({"action": "start", "pr_number": 8}));
        assert!(error);
        assert!(text.as_str().unwrap().contains("Only the user"), "{text}");
        assert_eq!(db.get_pr_watch("chat").unwrap().unwrap().pr_number, 7);

        // Once #7 is merged it can never wake again, so #8 may replace it.
        let mut merged = db.get_pr_watch("chat").unwrap().unwrap();
        merged.stop_reason = Some(crate::database::pr_watch::finished_reason("MERGED"));
        assert!(db.save_pr_watch_progress(&merged, &[], 3).unwrap());
        let (error, switched) = call(&host, &claude, json!({"action": "start", "pr_number": 8}));
        assert_eq!((error, switched["status"].as_str()), (false, Some("watching")));
        let switched = db.get_pr_watch("chat").unwrap().unwrap();
        assert_eq!((switched.pr_number, switched.attempts), (8, 0));
    }

    #[test]
    fn pr_watch_tool_stop_waits_for_a_wake_in_flight_so_a_refund_still_lands() {
        let (app, _dir, workspace) = app();
        let host = host(&app);
        let db = app.state::<DatabaseStore>();
        // Its own thread: the in-flight set is process-wide and tests run in
        // parallel.
        let thread = "chat-with-a-wake-in-flight";
        db.upsert_agent_chat_session(thread, &workspace, None, "claude").unwrap();
        let claude = HostCaller { thread_id: thread.into(), ..caller(ProviderKind::Claude, None) };
        call(&host, &claude, json!({"action": "start"}));
        let before = db.get_pr_watch(thread).unwrap().unwrap();

        // A sweep commits a wake (attempt spent, event told) and is delivering.
        let waking = crate::pr_watch::tests_claim_waking(thread);
        let mut woken = before.clone();
        woken.attempts = 1;
        assert!(db.save_pr_watch_progress(&woken, &["check:x".into()], 1).unwrap());
        let (error, text) = call(&host, &claude, json!({"action": "stop"}));
        assert!(error);
        assert!(text.as_str().unwrap().contains("Call stop again"), "{text}");
        assert_eq!(db.get_pr_watch(thread).unwrap().unwrap().status, "active");

        // The send was rejected before reaching the provider: the refund
        // still matches the generation, so the event will be told again.
        let mut refunded = woken.clone();
        refunded.attempts = 0;
        assert!(db.save_pr_watch_progress_unmarking(&refunded, &["check:x".into()], 2).unwrap());
        drop(waking);
        let (error, stopped) = call(&host, &claude, json!({"action": "stop"}));
        assert_eq!((error, stopped["status"].as_str()), (false, Some("stopped")));
        call(&host, &claude, json!({"action": "start"}));
        let resumed = db.get_pr_watch(thread).unwrap().unwrap();
        assert_eq!((resumed.status.as_str(), resumed.attempts), ("active", 0));
        assert!(!db.pr_watch_seen_keys(thread).unwrap().contains("check:x"));
    }

    #[test]
    fn pr_watch_tool_never_rearms_an_exhausted_watch() {
        let (app, _dir, workspace) = app();
        let host = host(&app);
        let db = app.state::<DatabaseStore>();
        let codex = caller(ProviderKind::Codex, Some("danger-full-access"));
        let (error, _) = call(&host, &codex, json!({"action": "start"}));
        assert!(!error);
        let mut exhausted = db.get_pr_watch("chat").unwrap().unwrap();
        exhausted.status = crate::database::pr_watch::PR_WATCH_EXHAUSTED.into();
        exhausted.attempts = exhausted.max_attempts;
        assert!(db.save_pr_watch_progress(&exhausted, &["check:a".into()], 1).unwrap());
        let exhausted = db.get_pr_watch("chat").unwrap().unwrap();

        // start → exhausted → stop → start, for this or another pull request.
        app.state::<AppStateStore>().update_workspace_prs(&workspace, vec![pr(7), pr(8)]);
        for args in [json!({"action": "start"}), json!({"action": "stop"}), json!({"action": "start", "pr_number": 8}), json!({"action": "start"})] {
            let (error, answer) = call(&host, &codex, args.clone());
            if args["action"] == "stop" {
                assert_eq!((error, answer["status"].as_str()), (false, Some("not_watching")));
            } else {
                assert!(error);
                assert!(answer.as_str().unwrap().contains("Only the user can re-arm"), "{answer}");
            }
        }
        assert_eq!(db.get_pr_watch("chat").unwrap().unwrap(), exhausted);
        assert!(db.pr_watch_seen_keys("chat").unwrap().contains("check:a"));

        // The user's re-arm in Codemux is what starts it over.
        let rearmed = db.arm_pr_watch("chat", &workspace, 7, exhausted.pr_url.as_deref(), 3, 2).unwrap();
        assert_eq!((rearmed.status.as_str(), rearmed.attempts, rearmed.wake_on_green), ("active", 0, false));
        let (error, again) = call(&host, &codex, json!({"action": "start"}));
        assert_eq!((error, again["status"].as_str()), (false, Some("already_watching")));
        assert!(db.get_pr_watch("chat").unwrap().unwrap().wake_on_green);
    }

    #[test]
    fn pr_watch_tool_watches_only_linked_local_github_pull_requests() {
        let (app, _dir, workspace) = app();
        let host = host(&app);
        let db = app.state::<DatabaseStore>();
        let claude = caller(ProviderKind::Claude, None);
        let state = app.state::<AppStateStore>();

        // Unlinked: asks Codemux to look the links up, at most once a minute.
        let before = REFRESHES.load(Ordering::SeqCst);
        for _ in 0..2 {
            let (error, text) = call(&host, &claude, json!({"action": "start", "pr_number": 9}));
            assert!(error);
            assert!(text.as_str().unwrap().contains("not linked"), "{text}");
        }
        assert_eq!(REFRESHES.load(Ordering::SeqCst), before + 1);

        state.update_workspace_prs(&workspace, vec![WorkspacePr {
            url: "https://gitlab.com/acme/app/-/merge_requests/7".into(),
            ..pr(7)
        }]);
        let (error, text) = call(&host, &claude, json!({"action": "start"}));
        assert!(error);
        assert!(text.as_str().unwrap().contains("GitHub pull requests only"), "{text}");

        state.update_workspace_prs(&workspace, vec![pr(7)]);
        let elsewhere = PrWatchHost { is_github: |_| Ok(false), ..host };
        let (error, text) = call(&elsewhere, &claude, json!({"action": "start"}));
        assert!(error);
        assert!(text.as_str().unwrap().contains("GitHub repositories only"), "{text}");
        assert!(db.get_pr_watch("chat").unwrap().is_none());
    }
}
