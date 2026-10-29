//! `request_secret`: an agent asks the user for an API key, token or
//! password in a private card above the composer instead of in the chat.
//!
//! The value never enters the transcript, events, logs or the model's
//! context. It goes straight from the card into an owner-only file, and the
//! agent only receives that file's path to read inside its own commands
//! (`TOKEN="$(cat <file>)" …`, `sudo -S -p '' … < <file>`). Files live in the
//! user's runtime directory when there is one (tmpfs on Linux) and are deleted
//! after 24 hours.
//!
//! Requests are in memory only: a restart forgets the open cards, and the
//! turn that asked has ended by then anyway.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::sync::watch;

use crate::mcp::registry::{HostCaller, HostTools};
use crate::mcp::runtime::{prefix_tool_name, McpTool};
use crate::state::AppStateStore;

const TOOL_NAME: &str = "request_secret";

/// `request_secret` as host tools are listed to agent chats.
pub const REQUEST_SECRET_REGISTRY_NAME: &str = "mcp__codemux__request_secret";

/// Fired with a chat's open requests whenever they change.
pub const SECRET_REQUESTS_CHANGED_EVENT: &str = "secret-requests-changed";

/// Session instructions for chats that list `request_secret`. Claude sees
/// MCP tools by name only until it searches for one, so without this an
/// agent asks the user to paste the secret into the chat.
pub const REQUEST_SECRET_GUIDANCE: &str = "## Secrets in Codemux\n\n\
When you need an API key, token, password or a sudo password from the user, call Codemux's `request_secret` tool \
(listed with an MCP prefix) instead of asking them to paste it into the chat or into a file, and never ask for the value in a message. \
The user types it into a private card, and you get back the path of an owner-only file that holds it. \
Read the file inside the command that needs it and never print, echo or log its contents.";

const TOOL_DESCRIPTION: &str = "Ask the user for a secret (API key, token, password, sudo password) in a private Codemux card, and wait for it. You never see the value: it is saved to an owner-only file and you get the file's path. Read it inside the command that needs it, e.g. `TOKEN=\"$(cat <file>)\" some-cli login` or `sudo -S -p '' <command> < <file>`, and never print it. The file is deleted after 24 hours. Asked how to provide a secret, call this now instead of explaining.

Each call waits up to about 50 seconds. If it returns `waiting`, the card is still open: call again with the returned `request_id` to keep waiting, and do not ask in chat. `declined`: the user chose not to. `cancelled` or `expired`: ask again only if you still need it.";

/// How long one call waits for the user. Below the 60 s every provider
/// allows a tool call, so a slow answer turns into `waiting`, not a failure.
const CALL_WAIT: Duration = Duration::from_secs(50);
/// How long a card stays open without an answer.
const REQUEST_LIFETIME_MS: i64 = 15 * 60 * 1000;
/// How long the agent can read a saved value.
const FILE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_LABEL_CHARS: usize = 120;
const MAX_REASON_CHARS: usize = 600;
const MAX_SECRET_BYTES: usize = 64 * 1024;

/// An open card, as the UI renders it. Never carries the value.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SecretRequestView {
    pub id: String,
    pub thread_id: String,
    pub label: String,
    pub reason: String,
    pub placeholder: Option<String>,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
struct SecretRequestsChanged {
    thread_id: String,
    requests: Vec<SecretRequestView>,
}

#[derive(Debug, Clone, PartialEq)]
enum Outcome {
    Saved(PathBuf),
    Declined,
    Cancelled,
    Expired,
}

struct Entry {
    view: SecretRequestView,
    /// The turn that asked, when the provider names it: only its end
    /// cancels the card.
    turn_id: Option<String>,
    outcome: watch::Sender<Option<Outcome>>,
}

impl Entry {
    fn is_open(&self) -> bool {
        self.outcome.borrow().is_none()
    }
}

/// Every chat's requests, open and answered. An answered one stays until it
/// would have expired, so every call waiting on it reads the same outcome.
#[derive(Default)]
struct Registry {
    entries: HashMap<String, Entry>,
}

fn registry() -> std::sync::MutexGuard<'static, Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolArgs {
    label: Option<String>,
    reason: Option<String>,
    placeholder: Option<String>,
    request_id: Option<String>,
}

/// What a call waits on: an existing request, or a new one.
enum Opened {
    Existing(String, watch::Receiver<Option<Outcome>>),
    Created(String, watch::Receiver<Option<Outcome>>),
}

fn bounded(field: &str, value: Option<&str>, max: usize) -> Result<Option<String>, String> {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if value.chars().count() > max {
        return Err(format!("{field} must be at most {max} characters."));
    }
    Ok(Some(value.to_string()))
}

impl Registry {
    /// Continue `request_id`, reuse the chat's open card with the same label
    /// (an agent retrying without its id), or open a new card.
    fn open(&mut self, thread_id: &str, turn_id: Option<&str>, args: &ToolArgs, now_ms: i64) -> Result<Opened, String> {
        self.prune(now_ms);
        if let Some(id) = args.request_id.as_deref() {
            return match self.entries.get(id) {
                Some(entry) if entry.view.thread_id == thread_id => {
                    Ok(Opened::Existing(id.to_string(), entry.outcome.subscribe()))
                }
                _ => Err("That request is unknown or has ended. Call request_secret again without request_id to ask again.".into()),
            };
        }
        let label = bounded("label", args.label.as_deref(), MAX_LABEL_CHARS)?
            .ok_or("Pass a label, such as \"OpenAI API key\" or \"sudo password\".")?;
        let reason = bounded("reason", args.reason.as_deref(), MAX_REASON_CHARS)?
            .ok_or("Pass a reason: what the secret is for and where the user can get it.")?;
        let placeholder = bounded("placeholder", args.placeholder.as_deref(), MAX_LABEL_CHARS)?;
        if let Some((id, entry)) = self
            .entries
            .iter()
            .find(|(_, e)| e.is_open() && e.view.thread_id == thread_id && e.view.label == label)
        {
            return Ok(Opened::Existing(id.clone(), entry.outcome.subscribe()));
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        let (tx, rx) = watch::channel(None);
        self.entries.insert(
            id.clone(),
            Entry {
                view: SecretRequestView {
                    id: id.clone(),
                    thread_id: thread_id.to_string(),
                    label,
                    reason,
                    placeholder,
                    expires_at_ms: now_ms + REQUEST_LIFETIME_MS,
                },
                turn_id: turn_id.map(str::to_string),
                outcome: tx,
            },
        );
        Ok(Opened::Created(id, rx))
    }

    /// Settle an open request. Returns its chat when it was still open.
    fn settle(&mut self, id: &str, outcome: Outcome) -> Option<String> {
        let entry = self.entries.get(id).filter(|e| e.is_open())?;
        entry.outcome.send_replace(Some(outcome));
        Some(entry.view.thread_id.clone())
    }

    /// Expire the open requests past their lifetime. Returns their chats.
    fn expire(&mut self, now_ms: i64) -> Vec<String> {
        let due: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, e)| e.is_open() && e.view.expires_at_ms <= now_ms)
            .map(|(id, _)| id.clone())
            .collect();
        due.iter().filter_map(|id| self.settle(id, Outcome::Expired)).collect()
    }

    /// Forget settled requests nobody collected.
    fn prune(&mut self, now_ms: i64) {
        self.entries
            .retain(|_, e| e.is_open() || e.view.expires_at_ms > now_ms);
    }

    /// Cancel the chat's open requests that `turn_id` asked, or whose turn
    /// is unknown.
    fn cancel_turn(&mut self, thread_id: &str, turn_id: &str) -> bool {
        let open: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, e)| {
                e.is_open()
                    && e.view.thread_id == thread_id
                    && e.turn_id.as_deref().is_none_or(|turn| turn == turn_id)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in &open {
            self.settle(id, Outcome::Cancelled);
        }
        !open.is_empty()
    }

    fn open_for(&self, thread_id: &str) -> Vec<SecretRequestView> {
        let mut open: Vec<SecretRequestView> = self
            .entries
            .values()
            .filter(|e| e.is_open() && e.view.thread_id == thread_id)
            .map(|e| e.view.clone())
            .collect();
        open.sort_by_key(|view| view.expires_at_ms);
        open
    }
}

/// Where saved values go: the user's runtime directory (tmpfs, owner-only,
/// cleared at logout) when the platform has one, else the app's data
/// directory.
fn secrets_dir() -> Option<PathBuf> {
    dirs::runtime_dir()
        .or_else(dirs::data_dir)
        .map(|root| root.join(crate::APP_DIR_NAME).join("secrets"))
}

/// Write `value` to a new owner-only file in `dir` and return its path.
fn write_private_file(dir: &Path, value: &str) -> Result<PathBuf, String> {
    let fail = |error: std::io::Error| format!("Could not save the secret: {}", error.kind());
    std::fs::create_dir_all(dir).map_err(fail)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(fail)?;
        options.mode(0o600);
    }
    let path = dir.join(uuid::Uuid::new_v4().simple().to_string());
    let mut file = options.open(&path).map_err(fail)?;
    if let Err(error) = std::io::Write::write_all(&mut file, value.as_bytes()) {
        drop(file);
        let _ = std::fs::remove_file(&path);
        return Err(fail(error));
    }
    Ok(path)
}

/// Delete saved values older than [`FILE_TTL`]. Returns how many it removed.
fn sweep_dir(dir: &Path, now: SystemTime) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let mut removed = 0;
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= FILE_TTL);
        if old && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Sweep expired files now and then every hour, for the app's lifetime.
pub fn spawn_sweeper() {
    tauri::async_runtime::spawn(async {
        let mut tick = tokio::time::interval(Duration::from_secs(60 * 60));
        loop {
            tick.tick().await;
            if let Some(dir) = secrets_dir() {
                sweep_dir(&dir, SystemTime::now());
            }
        }
    });
}

fn emit<R: Runtime>(app: &AppHandle<R>, thread_id: &str) {
    let requests = registry().open_for(thread_id);
    let _ = app.emit(
        SECRET_REQUESTS_CHANGED_EVENT,
        SecretRequestsChanged { thread_id: thread_id.to_string(), requests },
    );
}

/// A chat's turn ended: nobody is left to receive its answers.
pub fn end_turn<R: Runtime>(app: &AppHandle<R>, thread_id: &str, turn_id: &str) {
    if registry().cancel_turn(thread_id, turn_id) {
        emit(app, thread_id);
    }
}

fn expire_and_emit<R: Runtime>(app: &AppHandle<R>) {
    let expired = registry().expire(now_ms());
    for thread in expired {
        emit(app, &thread);
    }
}

// ── Commands ───────────────────────────────────────────────────────────

/// The chat's open cards, for a pane that mounts after they opened.
#[tauri::command]
pub fn agent_secret_requests(thread_id: String) -> Vec<SecretRequestView> {
    registry().open_for(&thread_id)
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SecretAnswer {
    Save { secret: String },
    Decline,
}

// Hand-written so a failed command can never print the value.
impl std::fmt::Debug for SecretAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Save { .. } => "Save",
            Self::Decline => "Decline",
        })
    }
}

#[tauri::command]
pub fn agent_secret_request_answer<R: Runtime>(
    app: AppHandle<R>,
    request_id: String,
    answer: SecretAnswer,
) -> Result<(), String> {
    expire_and_emit(&app);
    let thread_id = answer_request(&request_id, answer, secrets_dir())?;
    emit(&app, &thread_id);
    Ok(())
}

fn answer_request(id: &str, answer: SecretAnswer, dir: Option<PathBuf>) -> Result<String, String> {
    const ENDED: &str = "This request has ended. Ask the agent again if it still needs it.";
    let live = |entry: &Entry| entry.is_open() && entry.view.expires_at_ms > now_ms();
    if !registry().entries.get(id).is_some_and(live) {
        return Err(ENDED.into());
    }
    let outcome = match answer {
        SecretAnswer::Decline => Outcome::Declined,
        SecretAnswer::Save { secret } => {
            // Stored exactly as typed: a password may begin or end with a space.
            if secret.trim().is_empty() {
                return Err("Enter the secret, or decline.".into());
            }
            if secret.len() > MAX_SECRET_BYTES {
                return Err("That is too long for a secret.".into());
            }
            let dir = dir.ok_or("Codemux has no private folder to save secrets in.")?;
            sweep_dir(&dir, SystemTime::now());
            Outcome::Saved(write_private_file(&dir, &secret)?)
        }
    };
    let saved = match &outcome {
        Outcome::Saved(path) => Some(path.clone()),
        _ => None,
    };
    // Rechecked under the same lock as the settle: it may have expired or
    // ended while the file was written.
    let settled = {
        let mut reg = registry();
        if reg.entries.get(id).is_some_and(live) {
            reg.settle(id, outcome)
        } else {
            None
        }
    };
    match settled {
        Some(thread_id) => Ok(thread_id),
        // It ended while the file was written: nobody receives this path.
        None => {
            if let Some(path) = saved {
                let _ = std::fs::remove_file(path);
            }
            Err(ENDED.into())
        }
    }
}

// ── Agent-facing tool ──────────────────────────────────────────────────

fn tool() -> McpTool {
    McpTool {
        name: TOOL_NAME.into(),
        prefixed_name: prefix_tool_name("codemux", TOOL_NAME),
        server_id: "codemux-host".into(),
        description: Some(TOOL_DESCRIPTION.into()),
        input_schema: json!({
            "type": "object", "additionalProperties": false,
            "properties": {
                "label": {"type": "string", "maxLength": MAX_LABEL_CHARS, "description": "Card title, such as \"OpenAI API key\" or \"sudo password\"."},
                "reason": {"type": "string", "maxLength": MAX_REASON_CHARS, "description": "What it is for and where the user can get it."},
                "placeholder": {"type": "string", "maxLength": MAX_LABEL_CHARS, "description": "Optional input hint, such as \"sk-…\". Never a real value."},
                "request_id": {"type": "string", "description": "Keep waiting on a request an earlier call returned as `waiting`. Omit label and reason."}
            }
        }),
    }
}

fn now_ms() -> i64 {
    crate::pr_watch::now_ms()
}

fn result_text(value: Value, is_error: bool) -> Value {
    json!({"content": [{"type": "text", "text": value.as_str().map_or_else(|| value.to_string(), str::to_string)}], "isError": is_error})
}

fn outcome_result(outcome: Outcome) -> Value {
    match outcome {
        Outcome::Saved(path) => {
            let file = path.to_string_lossy();
            json!({
                "status": "saved",
                "file": file,
                "usage": format!("Read it inside the command that needs it and never print it: TOKEN=\"$(cat '{file}')\" some-cli …, or for sudo: sudo -S -p '' <command> < '{file}'. Deleted after 24 hours."),
            })
        }
        Outcome::Declined => json!({"status": "declined", "note": "The user chose not to provide it. Do not ask for it in chat."}),
        Outcome::Cancelled => json!({"status": "cancelled"}),
        Outcome::Expired => json!({"status": "expired", "note": "Nobody answered in time."}),
    }
}

/// Whether the chat's commands run on this computer, where the file is.
fn runs_locally<R: Runtime>(app: &AppHandle<R>, caller: &HostCaller) -> bool {
    let Some(workspace_id) = caller.workspace_id.as_deref() else { return false };
    app.try_state::<AppStateStore>()
        .and_then(|state| state.find_workspace(workspace_id))
        .is_some_and(|ws| ws.host_id.is_none() && ws.remote_cwd.is_none())
}

pub struct SecretRequestHost<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> SecretRequestHost<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self { app }
    }

    async fn answer(&self, caller: &HostCaller, turn_id: Option<&str>, arguments: &Value) -> Value {
        let args: ToolArgs = match serde_json::from_value(arguments.clone()) {
            Ok(args) => args,
            Err(error) => return result_text(json!(format!("Pass label and reason, or request_id: {error}")), true),
        };
        let thread_id = caller.thread_id.as_str();
        let opened = registry().open(thread_id, turn_id, &args, now_ms());
        let (id, mut rx) = match opened {
            Ok(Opened::Created(id, rx)) => {
                emit(&self.app, thread_id);
                // Closes the card on time even when the agent stops waiting.
                let app = self.app.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(REQUEST_LIFETIME_MS as u64)).await;
                    expire_and_emit(&app);
                });
                (id, rx)
            }
            Ok(Opened::Existing(id, rx)) => (id, rx),
            Err(error) => return result_text(json!(error), true),
        };
        let deadline = registry().entries.get(&id).map_or(0, |e| e.view.expires_at_ms);
        let wait = CALL_WAIT.min(Duration::from_millis((deadline - now_ms()).max(0) as u64));
        let _ = tokio::time::timeout(wait, rx.wait_for(Option::is_some)).await;
        expire_and_emit(&self.app);
        // Every call waiting on this request reads the same outcome.
        let outcome = rx.borrow().clone();
        match outcome {
            Some(outcome) => result_text(outcome_result(outcome), false),
            None => result_text(
                json!({"status": "waiting", "request_id": id, "note": "The card is still open. Call request_secret again with this request_id to keep waiting; do not ask in chat."}),
                false,
            ),
        }
    }
}

impl<R: Runtime> HostTools for SecretRequestHost<R> {
    fn tools_for(&self, caller: &HostCaller) -> Vec<McpTool> {
        // A delegated task has no one watching its pane; it reports to its
        // parent, which can ask.
        let is_child = self
            .app
            .try_state::<super::delegation::DelegationState>()
            .is_some_and(|state| state.is_running_child(&caller.thread_id));
        if is_child || !runs_locally(&self.app, caller) {
            return vec![];
        }
        vec![tool()]
    }

    fn call_async<'a>(
        &'a self,
        caller: &'a HostCaller,
        name: &'a str,
        args: &'a Value,
        _call_id: Option<&'a str>,
        run_id: Option<&'a str>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<Value>> + Send + 'a>> {
        Box::pin(async move {
            if name != REQUEST_SECRET_REGISTRY_NAME {
                return None;
            }
            if self.tools_for(caller).is_empty() {
                return Some(result_text(json!("request_secret is not available to this chat."), true));
            }
            Some(self.answer(caller, run_id, args).await)
        })
    }

    fn call(&self, _caller: &HostCaller, name: &str, _arguments: &Value) -> Option<Value> {
        // Waiting for the user needs the asynchronous route.
        (name == REQUEST_SECRET_REGISTRY_NAME)
            .then(|| result_text(json!("request_secret is not available to this chat."), true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(label: &str) -> ToolArgs {
        ToolArgs {
            label: Some(label.into()),
            reason: Some("To deploy".into()),
            placeholder: None,
            request_id: None,
        }
    }

    fn outcome(reg: &Registry, id: &str) -> Option<Outcome> {
        reg.entries.get(id)?.outcome.borrow().clone()
    }

    fn id_of(opened: Opened) -> (String, bool) {
        match opened {
            Opened::Created(id, _) => (id, true),
            Opened::Existing(id, _) => (id, false),
        }
    }

    #[test]
    fn a_retry_reuses_the_open_card_and_an_id_continues_it() {
        let mut reg = Registry::default();
        let (id, created) = id_of(reg.open("chat", None, &args("API key"), 0).unwrap());
        assert!(created);
        assert_eq!(id_of(reg.open("chat", None, &args("API key"), 1).unwrap()), (id.clone(), false));
        let resume = ToolArgs { request_id: Some(id.clone()), ..args("") };
        assert_eq!(id_of(reg.open("chat", None, &resume, 2).unwrap()), (id.clone(), false));
        // Another chat can neither see nor continue it.
        assert!(reg.open("other", None, &resume, 2).is_err());
        assert!(reg.open_for("other").is_empty());
        assert_eq!(reg.open_for("chat").len(), 1);
    }

    #[test]
    fn label_and_reason_are_required_and_bounded() {
        let mut reg = Registry::default();
        assert!(reg.open("chat", None, &ToolArgs { label: None, ..args("x") }, 0).is_err());
        assert!(reg.open("chat", None, &ToolArgs { reason: Some(" ".into()), ..args("x") }, 0).is_err());
        assert!(reg.open("chat", None, &args(&"x".repeat(MAX_LABEL_CHARS + 1)), 0).is_err());
    }

    #[test]
    fn settled_requests_leave_the_ui_and_every_waiter_reads_one_outcome() {
        let mut reg = Registry::default();
        let Opened::Created(id, first) = reg.open("chat", None, &args("Token"), 0).unwrap() else { panic!() };
        let Opened::Existing(_, second) = reg.open("chat", None, &args("Token"), 1).unwrap() else { panic!() };
        assert_eq!(reg.settle(&id, Outcome::Declined), Some("chat".into()));
        assert!(reg.open_for("chat").is_empty());
        // A second answer cannot overwrite the first.
        assert_eq!(reg.settle(&id, Outcome::Cancelled), None);
        assert_eq!(*first.borrow(), Some(Outcome::Declined));
        assert_eq!(*second.borrow(), Some(Outcome::Declined));
        // A later call resuming the id still reads it, until it would have expired.
        let resume = ToolArgs { request_id: Some(id.clone()), ..args("") };
        assert!(matches!(reg.open("chat", None, &resume, 2).unwrap(), Opened::Existing(..)));
        reg.prune(REQUEST_LIFETIME_MS);
        assert!(reg.open("chat", None, &resume, REQUEST_LIFETIME_MS).is_err());
    }

    #[test]
    fn requests_expire_and_end_with_their_own_turn() {
        let mut reg = Registry::default();
        let (old, _) = id_of(reg.open("chat", Some("t1"), &args("Old"), 0).unwrap());
        let (live, _) = id_of(reg.open("chat", Some("t2"), &args("Live"), REQUEST_LIFETIME_MS).unwrap());
        let (unknown, _) = id_of(reg.open("chat", None, &args("Unknown"), REQUEST_LIFETIME_MS).unwrap());
        assert_eq!(reg.expire(REQUEST_LIFETIME_MS), vec!["chat".to_string()]);
        assert_eq!(outcome(&reg, &old), Some(Outcome::Expired));
        // An earlier turn ending late leaves the next turn's card alone.
        assert!(reg.cancel_turn("chat", "t1"));
        assert_eq!(outcome(&reg, &live), None);
        assert_eq!(outcome(&reg, &unknown), Some(Outcome::Cancelled));
        assert!(reg.cancel_turn("chat", "t2"));
        assert!(!reg.cancel_turn("chat", "t2"));
        assert_eq!(outcome(&reg, &live), Some(Outcome::Cancelled));
    }

    #[tokio::test]
    async fn a_waiting_call_wakes_on_the_answer() {
        let mut reg = Registry::default();
        let Opened::Created(id, mut rx) = reg.open("chat", None, &args("Key"), 0).unwrap() else { panic!() };
        let tx = reg.entries[&id].outcome.clone();
        tokio::spawn(async move { tx.send_replace(Some(Outcome::Declined)) });
        tokio::time::timeout(Duration::from_secs(5), rx.wait_for(Option::is_some))
            .await
            .unwrap()
            .unwrap();
    }

    #[test]
    fn saved_values_are_owner_only_files_that_expire() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = dir.path().join("secrets");
        let path = write_private_file(&secrets, "hunter2").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hunter2");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(&secrets).unwrap().permissions().mode() & 0o777, 0o700);
        }
        assert_eq!(sweep_dir(&secrets, SystemTime::now()), 0);
        assert_eq!(sweep_dir(&secrets, SystemTime::now() + FILE_TTL), 1);
        assert!(!path.exists());
    }

    #[test]
    fn answering_writes_the_file_only_for_an_open_request() {
        let dir = tempfile::tempdir().unwrap();
        let id = {
            let mut reg = registry();
            id_of(reg.open("answer-chat", None, &args("Key"), now_ms()).unwrap()).0
        };
        let blank = answer_request(&id, SecretAnswer::Save { secret: "  ".into() }, Some(dir.path().into()));
        assert!(blank.is_err());
        let saved = answer_request(&id, SecretAnswer::Save { secret: " pass word ".into() }, Some(dir.path().into()));
        assert_eq!(saved.unwrap(), "answer-chat");
        let Some(Outcome::Saved(path)) = outcome(&registry(), &id) else { panic!("expected a saved file") };
        // Exactly as typed.
        assert_eq!(std::fs::read_to_string(path).unwrap(), " pass word ");
        // An ended request writes nothing.
        assert!(answer_request(&id, SecretAnswer::Save { secret: "sk-2".into() }, Some(dir.path().into())).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
