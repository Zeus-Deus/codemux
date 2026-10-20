//! Suggested follow-up tasks.
//!
//! A Claude or Codex chat calls the `suggest_tasks` host tool when it notices
//! work outside the current request. The call starts nothing: it returns at
//! once and the transcript shows the tasks as a card. From the card the user
//! can start a task (Start / Start all), do it in this chat instead (Do it
//! here, a plain user turn sent by the frontend), and later merge the
//! finished branches back (Merge all).
//!
//! A started task runs as its OWN workspace on its OWN git worktree, on a
//! new branch made from the parent workspace's current HEAD commit (committed
//! work only). Its chat is tracked by the delegation machinery
//! (`delegation::Placement::Worktree`): the same card row, idle detection,
//! journal and one results message per round. Unlike `delegate_task` it is
//! not capped and is not stopped by the parent's Stop or Close: it is an
//! independent workspace the user can watch. Closing or archiving the child
//! workspace stops it.
//!
//! State that must survive a reload lives in `agent_follow_up_tasks`
//! (`database::follow_ups`): which suggestion (tool call id + index) started
//! which child, its branch and base commit, its outcome and git stats, and
//! whether it was merged. The frontend reads it with `follow_up_list` and
//! refreshes on the `follow-up-changed` event.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::agent_provider::ProviderKind;
use crate::database::follow_ups::FollowUpTaskRow;
use crate::database::{AgentChatSessionConfig, DatabaseStore};
use crate::mcp::registry::HostCaller;
use crate::mcp::runtime::{prefix_tool_name, McpTool};
use crate::state::{AppStateStore, InitialChatPane};

use super::agent_chat::fallback_permission_mode;
use super::delegation::{
    excerpt, parse_provider, provider_id, provider_label, readable_error, DelegationState, Task,
    WorktreePlacement, CANDIDATES,
};
use super::usage_resume::now_ms;

pub const TOOL_NAME: &str = "suggest_tasks";
/// Event the frontend listens to; payload `{ parent_thread_id }`.
pub const FOLLOW_UP_CHANGED_EVENT: &str = "follow-up-changed";

const MAX_SUGGESTIONS: usize = 5;
const MIN_TASK_CHARS: usize = 20;
const MAX_TITLE_CHARS: usize = 60;
const MAX_TASK_CHARS: usize = 8_000;

/// Session instructions for chats that list `suggest_tasks`. Without them
/// agents list follow-ups in prose (Claude sees MCP tools by name only until
/// it searches for one), so the tool goes unused.
pub const SUGGEST_TASKS_GUIDANCE: &str = "## Suggesting follow-up tasks in Codemux\n\n\
Before your final reply, check whether you are leaving concrete work undone that is outside the request or too big to do now: \
a bug you noticed, docs that disagree with the code, missing tests, a needed refactor. If so, call Codemux's `suggest_tasks` tool \
(listed with an MCP prefix) with one task per item, then end your turn. Do not describe such work only in prose or ask the user \
whether to do it: the tool is how you raise it. The user sees each task as a card and can start it in its own worktree with one click; \
nothing starts on its own, so never wait for it. In your reply, mention the suggestions in at most one short sentence. \
Give each task a 3-8 word title and a self-contained brief another agent could do without this conversation. \
At most 5 tasks, never work you are about to do yourself.";

const TOOL_DESCRIPTION: &str = "Suggest follow-up tasks to the user without starting them. Use it unprompted when you notice work outside the current request or too large to do now, instead of listing follow-ups in prose. The user sees each task as a card and can start it in its own git worktree with a coding agent, ask you to do it here, or ignore it. It returns at once and starts nothing: do not wait, poll or repeat the tasks in your reply; continue or end your turn normally.\n\nEach task is read by an agent that sees nothing from this conversation: write a self-contained brief with the goal, the relevant files, constraints and how to check the result. It will work on a branch made from this workspace's latest commit, so say if it depends on uncommitted changes.";

// ── Pure rules ───────────────────────────────────────────────────────

/// One validated suggestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Suggestion {
    pub title: String,
    pub task: String,
    pub provider: Option<ProviderKind>,
}

#[derive(Debug, Deserialize)]
struct SuggestArgs {
    tasks: Vec<SuggestArg>,
}

#[derive(Debug, Deserialize)]
struct SuggestArg {
    #[serde(default)]
    title: String,
    task: String,
    #[serde(default)]
    provider: Option<String>,
}

/// Validate a `suggest_tasks` call's arguments. `Err` is the one-sentence
/// refusal the model sees.
pub(crate) fn parse_suggestions(arguments: &Value) -> Result<Vec<Suggestion>, String> {
    let args: SuggestArgs = serde_json::from_value(arguments.clone())
        .map_err(|_| "Pass `tasks`: an array of { title, task, provider? } objects.".to_string())?;
    if args.tasks.is_empty() {
        return Err("Suggest at least one task.".into());
    }
    if args.tasks.len() > MAX_SUGGESTIONS {
        return Err(format!(
            "Suggest at most {MAX_SUGGESTIONS} tasks at a time; keep the most useful ones."
        ));
    }
    args.tasks
        .into_iter()
        .enumerate()
        .map(|(index, arg)| {
            let task = arg.task.trim();
            if task.chars().count() < MIN_TASK_CHARS {
                return Err(format!(
                    "Task {} needs a complete, self-contained brief (at least {MIN_TASK_CHARS} characters).",
                    index + 1
                ));
            }
            let provider = match arg.provider.as_deref().map(str::trim) {
                None | Some("") => None,
                Some(id) => Some(
                    parse_provider(id)
                        .filter(|kind| CANDIDATES.contains(kind))
                        .ok_or_else(|| {
                            format!(
                                "Unknown agent \"{id}\" in task {}; use one of: {}, or omit it.",
                                index + 1,
                                CANDIDATES.map(provider_id).join(", ")
                            )
                        })?,
                ),
            };
            let flat_title = arg.title.split_whitespace().collect::<Vec<_>>().join(" ");
            let title = match flat_title.as_str() {
                "" => task.lines().next().unwrap_or_default().trim(),
                title => title,
            };
            Ok(Suggestion {
                title: excerpt(title, MAX_TITLE_CHARS, ""),
                task: excerpt(task, MAX_TASK_CHARS, "…"),
                provider,
            })
        })
        .collect()
}

/// Whether a provider-visible tool name is Codemux's `suggest_tasks`
/// (Claude: `mcp__codemux__mcp__codemux__suggest_tasks`, Codex:
/// `codemux_mcp__codemux__suggest_tasks`).
fn is_suggest_name(name: &str) -> bool {
    let scoped = format!("codemux__{TOOL_NAME}");
    name == TOOL_NAME || name == scoped || name.ends_with(&format!("__{scoped}"))
}

/// The arguments of a persisted tool call, if it is a `suggest_tasks` call.
/// Codex wraps host tools in a `dynamicToolCall` envelope.
pub(crate) fn suggest_arguments(tool_name: &str, input: &Value) -> Option<Value> {
    if is_suggest_name(tool_name) {
        return Some(input.clone());
    }
    if !matches!(tool_name, "dynamicToolCall" | "mcpToolCall") {
        return None;
    }
    let tool = input.get("tool").and_then(Value::as_str)?;
    if !is_suggest_name(tool) {
        return None;
    }
    match input.get("arguments")? {
        Value::String(text) => serde_json::from_str(text).ok(),
        other => Some(other.clone()),
    }
}

/// Whether `caller` is offered `suggest_tasks`: Claude and Codex chats in
/// any permission mode (it only suggests), never a running delegated or
/// started child.
fn offered(caller: &HostCaller, caller_is_child: bool) -> bool {
    !caller_is_child && matches!(caller.provider, ProviderKind::Claude | ProviderKind::Codex)
}

fn suggest_tool() -> McpTool {
    McpTool {
        name: TOOL_NAME.to_string(),
        prefixed_name: prefix_tool_name("codemux", TOOL_NAME),
        description: Some(TOOL_DESCRIPTION.to_string()),
        input_schema: json!({
            "type": "object",
            "properties": {
                "tasks": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": MAX_SUGGESTIONS,
                    "items": {
                        "type": "object",
                        "properties": {
                            "title": {
                                "type": "string",
                                "description": "3-8 word label shown on the card (max 60 characters)."
                            },
                            "task": {
                                "type": "string",
                                "description": "Complete, self-contained brief (at least 20 characters). The agent that runs it sees nothing else from this conversation."
                            },
                            "provider": {
                                "type": "string",
                                "enum": CANDIDATES.map(provider_id),
                                "description": "Optional: the agent best suited to it. The user can change it."
                            }
                        },
                        "required": ["title", "task"]
                    }
                }
            },
            "required": ["tasks"]
        }),
        server_id: "codemux-host".to_string(),
    }
}

/// The tool for `caller`, if it is offered.
pub(crate) fn tool_for<R: Runtime>(app: &AppHandle<R>, caller: &HostCaller) -> Option<McpTool> {
    let is_child = app
        .try_state::<DelegationState>()
        .is_some_and(|state| state.is_running_child(&caller.thread_id));
    offered(caller, is_child).then(suggest_tool)
}

/// Whether `prefixed_name` is this host tool.
pub(crate) fn is_tool(prefixed_name: &str) -> bool {
    prefixed_name == prefix_tool_name("codemux", TOOL_NAME)
}

/// Answer a `suggest_tasks` call: validate, start nothing, return at once.
pub(crate) fn call<R: Runtime>(
    app: &AppHandle<R>,
    caller: &HostCaller,
    arguments: &Value,
) -> Value {
    let is_child = app
        .try_state::<DelegationState>()
        .is_some_and(|state| state.is_running_child(&caller.thread_id));
    let outcome = if is_child {
        Err("A task started from a suggestion cannot suggest more tasks; mention them in your final report.".to_string())
    } else {
        parse_suggestions(arguments).map(|tasks| {
            json!({
                "suggested": tasks.len(),
                "note": "Shown to the user as cards. Nothing has started; the user decides. Do not wait for them or repeat them in your reply: continue your work, or end your turn as usual.",
            })
            .to_string()
        })
    };
    let (text, is_error) = match outcome {
        Ok(text) => (text, false),
        Err(refusal) => (refusal, true),
    };
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

/// The new branch's name: the title in kebab case, unique in the repo.
pub(crate) fn branch_name_for(title: &str, existing: &HashSet<String>) -> String {
    let name = crate::branch_name::sanitize_branch_name(&title.to_lowercase());
    let name = if name.is_empty() {
        "follow-up".to_string()
    } else {
        name
    };
    crate::branch_name::deconflict_against(&name, existing)
}

// ── Git ──────────────────────────────────────────────────────────────

fn git_command(dir: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null());
    #[cfg(test)]
    {
        // Tests never read the user's git config or write under HOME.
        let home = std::env::temp_dir().join("codemux-follow-up-test-home");
        let _ = std::fs::create_dir_all(&home);
        command
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Codemux Test")
            .env("GIT_AUTHOR_EMAIL", "test@codemux.invalid")
            .env("GIT_COMMITTER_NAME", "Codemux Test")
            .env("GIT_COMMITTER_EMAIL", "test@codemux.invalid");
    }
    command
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = git_command(dir)
        .args(args)
        .output()
        .map_err(|e| format!("Failed to run git: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// The commit a child branches from: the parent's checked-out HEAD.
/// Uncommitted edits in the parent are deliberately not carried.
pub(crate) fn head_sha(cwd: &Path) -> Result<String, String> {
    git(cwd, &["rev-parse", "--verify", "HEAD^{commit}"]).map_err(|_| {
        "This workspace has no commit to branch from yet. Commit first, then start the task."
            .to_string()
    })
}

/// Files with uncommitted changes. `include_untracked` also counts new files.
fn dirty_files(cwd: &Path, include_untracked: bool) -> Result<Vec<String>, String> {
    let untracked = if include_untracked {
        "--untracked-files=all"
    } else {
        "--untracked-files=no"
    };
    let out = git(cwd, &["status", "--porcelain", untracked])?;
    Ok(out
        .lines()
        .filter(|line| line.len() > 3)
        .map(|line| line[3..].to_string())
        .collect())
}

/// `(additions, deletions, commits)` on `HEAD` since `base`.
fn branch_stats(worktree: &Path, base: &str) -> Result<(u32, u32, u32), String> {
    let range = format!("{base}..HEAD");
    let numstat = git(worktree, &["diff", "--numstat", &format!("{base}...HEAD")])?;
    let (mut additions, mut deletions) = (0u32, 0u32);
    for line in numstat.lines() {
        let mut parts = line.split_whitespace();
        additions += parts
            .next()
            .and_then(|n| n.parse::<u32>().ok())
            .unwrap_or(0);
        deletions += parts
            .next()
            .and_then(|n| n.parse::<u32>().ok())
            .unwrap_or(0);
    }
    let commits = git(worktree, &["rev-list", "--count", &range])?
        .trim()
        .parse()
        .unwrap_or(0);
    Ok((additions, deletions, commits))
}

/// One branch to merge, in order.
#[derive(Debug, Clone)]
pub(crate) struct MergeCandidate {
    pub child_thread_id: String,
    pub title: String,
    pub branch: String,
    /// The child's worktree, checked for uncommitted work first.
    pub worktree: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MergedBranch {
    pub child_thread_id: String,
    pub title: String,
    pub branch: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SkippedBranch {
    pub child_thread_id: String,
    pub title: String,
    pub branch: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MergeConflict {
    pub child_thread_id: String,
    pub title: String,
    pub branch: String,
    pub files: Vec<String>,
    /// Git's own words when the merge failed for another reason.
    pub message: Option<String>,
}

/// What Merge all did. Merging stops at the first conflict, which is
/// aborted, so `merged` lists only branches that are in. `stopped` says why
/// it stopped early for another reason (someone else's merge appeared);
/// whatever merged before that is still listed, so its state is kept.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
pub struct MergeAllResult {
    pub target_branch: Option<String>,
    pub merged: Vec<MergedBranch>,
    pub skipped: Vec<SkippedBranch>,
    pub conflict: Option<MergeConflict>,
    pub stopped: Option<String>,
}

const MERGE_IN_PROGRESS: &str =
    "A merge is already in progress in this workspace. Finish or abort it first.";
const MERGE_APPEARED: &str =
    "Stopped: another merge started in this workspace. It was left as it is; finish or abort it, then merge the rest.";

/// Checkouts Merge all is merging into right now, by git directory, so two
/// app-initiated runs never interleave in one checkout.
static MERGING: std::sync::LazyLock<std::sync::Mutex<HashSet<PathBuf>>> =
    std::sync::LazyLock::new(Default::default);

struct MergeLock(PathBuf);

impl MergeLock {
    fn acquire(target: &Path) -> Result<Self, String> {
        let key = git(target, &["rev-parse", "--absolute-git-dir"])
            .map(PathBuf::from)
            .unwrap_or_else(|_| target.to_path_buf());
        let key = key.canonicalize().unwrap_or(key);
        let mut held = MERGING.lock().unwrap_or_else(|p| p.into_inner());
        if !held.insert(key.clone()) {
            return Err("Merge all is already running in this workspace.".into());
        }
        Ok(Self(key))
    }
}

impl Drop for MergeLock {
    fn drop(&mut self) {
        MERGING
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.0);
    }
}

/// The commit `MERGE_HEAD` names in the checkout at `dir`, if a merge is in
/// progress.
fn merge_head(dir: &Path) -> Option<String> {
    git(dir, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).ok()
}

/// Run `git merge`, keeping its exit code: 1 means the merge ran and
/// stopped on conflicts; a refusal to start (an unfinished merge, unmerged
/// files, local changes in the way) exits with another code.
fn git_merge(dir: &Path, args: &[&str]) -> Result<(), (Option<i32>, String)> {
    let output = git_command(dir)
        .arg("merge")
        .args(args)
        .output()
        .map_err(|e| (None, format!("Failed to run git: {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Err((output.status.code(), if stderr.is_empty() { stdout } else { stderr }))
}

/// Whether a failed `git merge <theirs>` left a merge that this run owns:
/// git ran the merge and stopped on conflicts (exit 1), and the merge in
/// progress is of exactly the commit this run asked for. A refusal to start
/// means the merge in progress belongs to someone else, even when it is of
/// the same branch; ownership that is uncertain is never assumed.
fn owns_failed_merge(dir: &Path, exit_code: Option<i32>, theirs: &str) -> bool {
    exit_code == Some(1) && merge_head(dir).as_deref() == Some(theirs)
}

/// Whether the checkout containing `dir` has a merge in progress. Asked
/// through git so it holds in a linked worktree (where `.git` is a file)
/// and from a subdirectory of the checkout.
fn merge_in_progress(dir: &Path) -> bool {
    merge_head(dir).is_some()
}

/// Merge `candidates` into the checkout at `target` with `--no-ff`, in
/// order. Refuses a target with a merge in progress or uncommitted changes.
/// A child with uncommitted work in its worktree is reported as not
/// mergeable, never merged half-done. On a conflict the merge this run
/// started is aborted and nothing after it is attempted; a merge it did not
/// start is never aborted. A merge someone else starts mid-run ends the run
/// with `stopped`, keeping what already merged.
pub(crate) fn merge_branches(
    target: &Path,
    candidates: &[MergeCandidate],
) -> Result<MergeAllResult, String> {
    merge_branches_with(target, candidates, |_| {})
}

/// [`merge_branches`] with a hook run after each candidate's last preflight
/// check and just before its merge, so tests can interleave another
/// process's git operation in that window.
fn merge_branches_with(
    target: &Path,
    candidates: &[MergeCandidate],
    mut before_merge: impl FnMut(&MergeCandidate),
) -> Result<MergeAllResult, String> {
    let _lock = MergeLock::acquire(target)?;
    if merge_in_progress(target) {
        return Err(MERGE_IN_PROGRESS.into());
    }
    let dirty = dirty_files(target, false)?;
    if !dirty.is_empty() {
        return Err(format!(
            "This workspace has {} uncommitted change{}. Commit or stash {} first, then merge.",
            dirty.len(),
            if dirty.len() == 1 { "" } else { "s" },
            if dirty.len() == 1 { "it" } else { "them" },
        ));
    }
    let mut result = MergeAllResult {
        target_branch: crate::git::current_branch(target),
        ..MergeAllResult::default()
    };
    for candidate in candidates {
        let skip = |reason: String| SkippedBranch {
            child_thread_id: candidate.child_thread_id.clone(),
            title: candidate.title.clone(),
            branch: candidate.branch.clone(),
            reason,
        };
        if let Some(worktree) = candidate.worktree.as_deref().filter(|path| path.exists()) {
            let left = dirty_files(worktree, true).unwrap_or_default();
            if !left.is_empty() {
                result.skipped.push(skip(format!(
                    "Not mergeable: {} uncommitted file{} in its worktree.",
                    left.len(),
                    if left.len() == 1 { "" } else { "s" }
                )));
                continue;
            }
        }
        if git(
            target,
            &[
                "rev-parse",
                "--verify",
                &format!("refs/heads/{}", candidate.branch),
            ],
        )
        .is_err()
        {
            result
                .skipped
                .push(skip("Its branch no longer exists.".to_string()));
            continue;
        }
        let ahead = git(
            target,
            &[
                "rev-list",
                "--count",
                &format!("HEAD..{}", candidate.branch),
            ],
        )
        .ok()
        .and_then(|n| n.trim().parse::<u32>().ok())
        .unwrap_or(0);
        if ahead == 0 {
            result
                .skipped
                .push(skip("No new commits to merge.".to_string()));
            continue;
        }
        let message = format!("Merge {}: {}", candidate.branch, candidate.title);
        // Someone may have started a merge since the check above. Stop
        // without touching it, and keep what already merged.
        if merge_in_progress(target) {
            result.stopped = Some(MERGE_APPEARED.into());
            break;
        }
        before_merge(candidate);
        // The commit this merge asks for: a merge in progress afterwards is
        // ours only if it names exactly this commit.
        let theirs = git(
            target,
            &["rev-parse", "--verify", &format!("refs/heads/{}^{{commit}}", candidate.branch)],
        )
        .unwrap_or_default();
        match git_merge(
            target,
            &["--no-ff", "--no-edit", "-m", &message, &candidate.branch],
        ) {
            Ok(_) => result.merged.push(MergedBranch {
                child_thread_id: candidate.child_thread_id.clone(),
                title: candidate.title.clone(),
                branch: candidate.branch.clone(),
            }),
            Err((exit_code, error)) => {
                if merge_in_progress(target) && !owns_failed_merge(target, exit_code, &theirs) {
                    // Someone else's merge got in between our check and
                    // ours. Never abort it, and keep its resolution work.
                    result.stopped = Some(MERGE_APPEARED.into());
                    break;
                }
                let files: Vec<String> = git(target, &["diff", "--name-only", "--diff-filter=U"])
                    .unwrap_or_default()
                    .lines()
                    .map(str::to_string)
                    .collect();
                // Leave the workspace exactly as it was before this merge.
                if merge_in_progress(target) {
                    let _ = git(target, &["merge", "--abort"]);
                }
                result.conflict = Some(MergeConflict {
                    child_thread_id: candidate.child_thread_id.clone(),
                    title: candidate.title.clone(),
                    branch: candidate.branch.clone(),
                    message: files
                        .is_empty()
                        .then(|| error.lines().next().unwrap_or("").to_string()),
                    files,
                });
                break;
            }
        }
    }
    Ok(result)
}

// ── App glue ─────────────────────────────────────────────────────────

pub(crate) fn emit_changed<R: Runtime>(app: &AppHandle<R>, parent_thread_id: &str) {
    let _ = app.emit(
        FOLLOW_UP_CHANGED_EVENT,
        json!({ "parent_thread_id": parent_thread_id }),
    );
}

/// The card row a started task fills: `<tool call id>#<index>`.
pub(crate) fn placement_of(row: &FollowUpTaskRow) -> WorktreePlacement {
    WorktreePlacement {
        branch: row.branch.clone(),
        suggestion: format!("{}#{}", row.tool_call_id, row.index),
    }
}

/// A started task finished (the delegation tracker calls this once, from
/// `finish`): store its outcome, then its git stats, and tell the card.
pub(crate) fn on_task_finished<R: Runtime>(app: &AppHandle<R>, child: &str, outcome: &str) {
    let Some(db) = app.try_state::<DatabaseStore>() else {
        return;
    };
    if let Err(error) = db.set_follow_up_outcome(child, outcome) {
        eprintln!("[codemux::follow_ups] {error}");
    }
    let Some(row) = db.get_follow_up_task(child) else {
        return;
    };
    emit_changed(app, &row.parent_thread_id);
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let worktree = PathBuf::from(&row.worktree_path);
        if row.worktree_path.is_empty() || !worktree.exists() {
            return;
        }
        let stats = branch_stats(&worktree, &row.base_sha);
        let uncommitted = dirty_files(&worktree, true)
            .ok()
            .map(|files| files.len() as u32);
        if let (Ok((additions, deletions, commits)), Some(db)) =
            (stats, app.try_state::<DatabaseStore>())
        {
            if let Err(error) = db.set_follow_up_stats(
                &row.child_thread_id,
                additions,
                deletions,
                commits,
                uncommitted,
            ) {
                eprintln!("[codemux::follow_ups] {error}");
            }
        }
        emit_changed(&app, &row.parent_thread_id);
    });
}

/// Children whose `follow_up_start` is still between claiming the
/// suggestion and handing the task to the delegation tracker, in this
/// process. Startup reconciliation must not touch them.
static STARTING: std::sync::LazyLock<std::sync::Mutex<HashSet<String>>> =
    std::sync::LazyLock::new(Default::default);

fn starting() -> std::sync::MutexGuard<'static, HashSet<String>> {
    STARTING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Marks a child as starting for as long as it lives.
struct StartingGuard(String);

impl StartingGuard {
    fn new(child: &str) -> Self {
        starting().insert(child.to_string());
        Self(child.to_string())
    }
}

impl Drop for StartingGuard {
    fn drop(&mut self) {
        starting().remove(&self.0);
    }
}

/// Settle rows a previous run left `running` without ever handing them to
/// the delegation tracker (quit or crash during `follow_up_start`, before
/// its restart journal entry was written). Rows the tracker knows (`is_live`)
/// or that this process is starting right now are left alone. A start that
/// never got its workspace gives the suggestion back so it can be started
/// again; one that did is Stopped, like any task the app closed on.
/// Returns the parents whose rows changed.
pub(crate) fn reconcile_interrupted_starts(
    db: &DatabaseStore,
    is_live: impl Fn(&str) -> bool,
) -> Vec<String> {
    let mut parents = Vec::new();
    for row in db.running_follow_up_tasks() {
        let child = row.child_thread_id.as_str();
        if is_live(child) || starting().contains(child) {
            continue;
        }
        let settled = if row.workspace_id.is_empty() {
            db.delete_follow_up_task(child)
        } else {
            db.set_follow_up_outcome(child, "stopped")
        };
        match settled {
            Ok(()) if !parents.contains(&row.parent_thread_id) => {
                parents.push(row.parent_thread_id)
            }
            Ok(()) => {}
            Err(error) => eprintln!("[codemux::follow_ups] {error}"),
        }
    }
    parents
}

/// [`reconcile_interrupted_starts`] at launch, after the delegation journal
/// was drained (journaled children are already Stopped by then).
pub(crate) fn reconcile_on_launch<R: Runtime>(app: &AppHandle<R>) {
    let Some(db) = app.try_state::<DatabaseStore>() else {
        return;
    };
    let state = app.try_state::<DelegationState>();
    let parents = reconcile_interrupted_starts(&db, |child| {
        state
            .as_ref()
            .is_some_and(|state| state.is_running_child(child))
    });
    for parent in parents {
        emit_changed(app, &parent);
    }
}

/// An agent that can run a started task, for the card's picker.
#[derive(Debug, Clone, Serialize)]
pub struct FollowUpProvider {
    pub id: String,
    pub label: String,
    pub installed: bool,
}

/// Claude and Codex always, then every other delegation candidate whose
/// CLI is on `PATH`.
#[tauri::command]
pub async fn follow_up_providers() -> Vec<FollowUpProvider> {
    CANDIDATES
        .into_iter()
        .filter_map(|kind| {
            let installed = which::which(super::provider_updates::binary_name(kind)).is_ok();
            let always = matches!(kind, ProviderKind::Claude | ProviderKind::Codex);
            (always || installed).then(|| FollowUpProvider {
                id: provider_id(kind).to_string(),
                label: provider_label(kind).to_string(),
                installed,
            })
        })
        .collect()
}

/// Every task started from `parent_thread_id`'s suggestions.
#[tauri::command]
pub fn follow_up_list(
    db: tauri::State<'_, DatabaseStore>,
    parent_thread_id: String,
) -> Vec<FollowUpTaskRow> {
    db.list_follow_up_tasks(&parent_thread_id)
}

#[derive(Debug, Clone, Serialize)]
pub struct FollowUpStarted {
    pub child_thread_id: String,
    pub workspace_id: String,
    pub branch: String,
}

/// The parent chat's workspace, as it is now.
fn parent_workspace(
    app_state: &AppStateStore,
    db: &DatabaseStore,
    parent: &str,
) -> Result<crate::state::WorkspaceSnapshot, String> {
    app_state
        .agent_chat_pane_id_for_thread(parent)
        .and_then(|pane| app_state.workspace_id_for_pane(&pane))
        .or_else(|| {
            db.get_agent_chat_session(parent)
                .map(|record| record.workspace_id)
        })
        .and_then(|id| app_state.find_workspace(&id))
        .ok_or_else(|| "This chat has no open workspace.".to_string())
}

fn parent_cwd(
    db: &DatabaseStore,
    parent: &str,
    workspace: &crate::state::WorkspaceSnapshot,
) -> PathBuf {
    PathBuf::from(
        db.get_agent_chat_session(parent)
            .and_then(|record| record.cwd)
            .unwrap_or_else(|| workspace.cwd.clone()),
    )
}

/// Admission can lose the race with Stop while git preparation runs. Release
/// the suggestion claim so the next explicit Start can try again.
fn admit_follow_up_intent(db: &DatabaseStore, row: &FollowUpTaskRow, intent: &crate::database::delegation_tasks::TaskRecord) -> Result<(), String> {
    if let Err(error) = db.admit_delegation_task(intent, None, &json!({"suggestion":row.tool_call_id,"index":row.index,"child":row.child_thread_id})) {
        db.delete_follow_up_task(&row.child_thread_id)?;
        return Err(error);
    }
    Ok(())
}

/// Start a persisted suggestion in its own worktree from the parent's HEAD.
#[tauri::command]
pub async fn follow_up_start<R: Runtime>(
    app: AppHandle<R>,
    parent_thread_id: String,
    tool_call_id: String,
    index: u32,
    provider: String,
    model: Option<String>,
    effort: Option<String>,
) -> Result<FollowUpStarted, String> {
    let db = app.state::<DatabaseStore>();
    let app_state = app.state::<AppStateStore>();
    let parent = parent_thread_id.as_str();
    let admission_generation = db.delivery_generation(parent)?.0;
    let provider = parse_provider(&provider)
        .filter(|kind| CANDIDATES.contains(kind))
        .ok_or_else(|| format!("Codemux cannot start a task with \"{provider}\"."))?;

    // The suggestion, from the parent's own persisted transcript.
    let stored = db
        .find_tool_use_input(parent, &tool_call_id)
        .ok_or_else(|| "This suggestion is no longer in the chat's history.".to_string())?;
    let arguments = suggest_arguments(
        stored["tool_name"].as_str().unwrap_or_default(),
        &stored["input"],
    )
    .ok_or_else(|| "That tool call is not a task suggestion.".to_string())?;
    let suggestion = parse_suggestions(&arguments)?
        .into_iter()
        .nth(index as usize)
        .ok_or_else(|| "That suggestion does not exist.".to_string())?;
    if db
        .get_follow_up_for_suggestion(parent, &tool_call_id, index)
        .is_some()
    {
        return Err("This task was already started.".into());
    }

    let workspace = parent_workspace(&app_state, &db, parent)?;
    if workspace.host_id.is_some() || workspace.attach_only {
        return Err(
            "Suggested tasks run on this computer; this chat's workspace is on another device."
                .into(),
        );
    }
    let cwd = parent_cwd(&db, parent, &workspace);
    let (repo_path, base_sha, parent_branch, existing) = {
        let cwd = cwd.clone();
        tokio::task::spawn_blocking(move || -> Result<_, String> {
            let repo = crate::config::workspace_config::find_git_root(&cwd).ok_or_else(|| {
                "This workspace is not a git repository, so a task cannot get its own worktree."
                    .to_string()
            })?;
            let sha = head_sha(&cwd)?;
            let mut existing: HashSet<String> = HashSet::new();
            existing.extend(crate::git::git_list_branches(&repo, false).unwrap_or_default());
            for branch in crate::git::git_list_branches(&repo, true).unwrap_or_default() {
                existing.insert(
                    branch
                        .strip_prefix("origin/")
                        .unwrap_or(&branch)
                        .to_string(),
                );
            }
            Ok((repo, sha, crate::git::current_branch(&cwd), existing))
        })
        .await
        .map_err(|e| format!("follow-up start task join failed: {e}"))??
    };
    let branch = branch_name_for(&suggestion.title, &existing);
    let child = uuid::Uuid::new_v4().to_string();
    let _starting = StartingGuard::new(&child);

    // Claims the suggestion before the slow part, so a second click fails.
    // If the app quits before the journal entry below, the next launch
    // settles the row (`reconcile_on_launch`).
    let mut row = FollowUpTaskRow {
        child_thread_id: child.clone(),
        parent_thread_id: parent.to_string(),
        tool_call_id: tool_call_id.clone(),
        index,
        workspace_id: String::new(),
        parent_workspace_id: workspace.workspace_id.0.clone(),
        repo_path: repo_path.display().to_string(),
        worktree_path: String::new(),
        branch: branch.clone(),
        base_sha: base_sha.clone(),
        provider: provider_id(provider).to_string(),
        title: suggestion.title.clone(),
        outcome: "running".into(),
        additions: None,
        deletions: None,
        commits: None,
        uncommitted: None,
        merged: false,
        created_at_ms: now_ms(),
    };
    db.insert_follow_up_task(&row)
        .map_err(|_| "This task was already started.".to_string())?;
    emit_changed(&app, parent);
    // Persist ownership before setup can await. A parent Stop must fence a
    // worktree child even before its workspace or provider session exists.
    let mut intent=crate::database::delegation_tasks::TaskRecord::new(format!("local:{}",uuid::Uuid::new_v4()),parent.into(),Some(child.clone()),provider_id(provider).into(),suggestion.title.clone(),suggestion.task.clone(),row.created_at_ms);
    intent.parent_title=db.get_agent_chat_title(parent);intent.workspace_id=workspace.workspace_id.0.clone();intent.cwd=repo_path.display().to_string();intent.isolation="worktree".into();intent.preparation_state="preparing".into();intent.tool_call_id=Some(row.tool_call_id.clone());
    intent.admission_generation = Some(admission_generation);
    if let Err(error) = admit_follow_up_intent(&db, &row, &intent) {
        emit_changed(&app, parent);
        return Err(error);
    }

    let created = super::workspace::create_worktree_workspace_impl_with_selection(
        app.clone(),
        &app_state,
        &db,
        &app.state::<crate::terminal::PtyState>(),
        &app.state::<crate::presets::PresetStoreState>(),
        repo_path.display().to_string(),
        branch.clone(),
        true,
        Some(base_sha.clone()),
        "empty".into(),
        None,
        None,
        None,
        None,
        false,
        Some(InitialChatPane {
            provider,
            thread_id: child.clone(),
        }),
    )
    .await;
    let created = match created {
        Ok(created) if !created.adopted => created,
        Ok(_) => {
            super::delegation::preparation_failed(&app,&intent.task_id,&format!("A workspace for branch {branch} is already open."));
            let _ = db.delete_follow_up_task(&child);
            emit_changed(&app, parent);
            return Err(format!("A workspace for branch {branch} is already open."));
        }
        Err(error) => {
            super::delegation::preparation_failed(&app,&intent.task_id,&format!("Codemux could not create its worktree: {}",readable_error(&error)));
            let _ = db.delete_follow_up_task(&child);
            emit_changed(&app, parent);
            return Err(format!(
                "Codemux could not create its worktree: {}",
                readable_error(&error)
            ));
        }
    };
    let workspace_id = created.workspace_id;
    let worktree_path = app_state.workspace_cwd(&workspace_id).unwrap_or_default();
    row.workspace_id = workspace_id.clone();
    row.worktree_path = worktree_path.clone();
    let _ = db.set_follow_up_placement(&child, &workspace_id, &worktree_path);
    app_state.set_workspace_parent(&workspace_id, Some(workspace.workspace_id.0.clone()));
    app_state.rename_workspace(&workspace_id, suggestion.title.clone());
    // Diffs and the sidebar read the parent's branch, not a bare commit.
    if parent_branch.is_some() {
        app_state.set_workspace_base_branch(&workspace_id, parent_branch);
    }

    // The child's chat row first, so Open and its pickers work before the
    // session exists, and its title is not auto-derived.
    let model = model
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty());
    let effort = effort
        .map(|e| e.trim().to_string())
        .filter(|e| !e.is_empty());
    let config = AgentChatSessionConfig {
        model: Some(model.clone()),
        effort: Some(effort.clone()),
        context_window: Some(None),
        permission_mode: Some(fallback_permission_mode(provider).map(str::to_string)),
        fast_mode: Some(false),
        service_tier: None,
    };
    let rows = db
        .upsert_agent_chat_session(
            &child,
            &workspace_id,
            Some(&worktree_path),
            provider_id(provider),
        )
        .and_then(|()| db.update_agent_chat_session_config(&child, &config))
        .and_then(|()| db.set_agent_chat_title(&child, &suggestion.title));
    crate::state::emit_app_state(&app);
    let pane_id = app_state.agent_chat_pane_id_for_thread(&child);
    let (Ok(()), Some(pane_id)) = (rows, pane_id) else {
        super::delegation::preparation_failed(&app,&intent.task_id,"Codemux created the workspace but could not open its chat.");
        let _ = db.set_follow_up_outcome(&child, "failed");
        emit_changed(&app, parent);
        return Err("Codemux created the workspace but could not open its chat.".into());
    };

    let task = Task::new(
        parent,
        provider,
        &suggestion.title,
        &suggestion.task,
        row.created_at_ms,
    )
    .with_identity(intent.task_id.clone())
    .with_model(model, effort)
    .in_worktree(placement_of(&row));
    let state = app.state::<DelegationState>();
    state.reserve_worktree(&child, task.clone());
    let mut durable=super::delegation::record_of(&child,&task);
    durable.workspace_id=workspace_id.clone();durable.cwd=worktree_path.clone();
    durable.permission_mode=fallback_permission_mode(provider).map(str::to_string);
    db.update_delegation_task(&intent.task_id,|t|{t.workspace_id=durable.workspace_id.clone();t.cwd=durable.cwd.clone();t.model=durable.model.clone();t.effort=durable.effort.clone();t.permission_mode=durable.permission_mode.clone();t.worktree=Some(json!({"path":worktree_path,"branch":row.branch,"base_ref":row.base_sha}));})?;
    if db.delegation_task(&intent.task_id)?.is_some_and(|t|t.cancellation_generation>0) {
        super::delegation::on_thread_stopped(&app,&child,super::delegation::StopKind::Stop);
        super::delegation::stop_confirmed(&app,&child);
        return Err("Task stopped during worktree preparation; its workspace is retained".into());
    }
    state.emit_card(&app, &child);
    emit_changed(&app, parent);
    tauri::async_runtime::spawn(super::delegation::run_starter(
        app.clone(),
        child.clone(),
        pane_id,
        PathBuf::from(&worktree_path),
    ));
    Ok(FollowUpStarted {
        child_thread_id: child,
        workspace_id,
        branch,
    })
}

/// Merge every finished, unmerged child branch of `parent_thread_id` into
/// the parent's checkout, oldest first. Off the main thread; see
/// [`merge_branches`] for the rules.
#[tauri::command]
pub async fn follow_up_merge_all<R: Runtime>(
    app: AppHandle<R>,
    parent_thread_id: String,
) -> Result<MergeAllResult, String> {
    let db = app.state::<DatabaseStore>();
    let app_state = app.state::<AppStateStore>();
    let workspace = parent_workspace(&app_state, &db, &parent_thread_id)?;
    let target = parent_cwd(&db, &parent_thread_id, &workspace);
    let rows = db.list_follow_up_tasks(&parent_thread_id);
    let mut skipped = Vec::new();
    let mut candidates = Vec::new();
    for row in rows.iter().filter(|row| !row.merged) {
        if row.outcome == "completed" {
            candidates.push(MergeCandidate {
                child_thread_id: row.child_thread_id.clone(),
                title: row.title.clone(),
                branch: row.branch.clone(),
                worktree: (!row.worktree_path.is_empty())
                    .then(|| PathBuf::from(&row.worktree_path)),
            });
        } else {
            skipped.push(SkippedBranch {
                child_thread_id: row.child_thread_id.clone(),
                title: row.title.clone(),
                branch: row.branch.clone(),
                reason: match row.outcome.as_str() {
                    "running" => "Still running.".to_string(),
                    "failed" => "It failed; open it to see why.".to_string(),
                    _ => "It was stopped before it finished.".to_string(),
                },
            });
        }
    }
    if candidates.is_empty() {
        return Err("No finished tasks left to merge.".into());
    }
    let mut result = tokio::task::spawn_blocking(move || merge_branches(&target, &candidates))
        .await
        .map_err(|e| format!("merge task join failed: {e}"))??;
    for merged in &result.merged {
        if let Err(error) = db.set_follow_up_merged(&merged.child_thread_id) {
            eprintln!("[codemux::follow_ups] {error}");
        }
    }
    result.skipped.extend(skipped);
    emit_changed(&app, &parent_thread_id);
    Ok(result)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tempfile::TempDir;

    fn caller(provider: ProviderKind, mode: Option<&str>) -> HostCaller {
        HostCaller {
            thread_id: "parent".into(),
            provider,
            workspace_id: Some("ws".into()),
            permission_mode: mode.map(str::to_string),
        }
    }

    fn brief(text: &str) -> String {
        format!("{text} in src/lib/strings.ts with table tests.")
    }

    #[test]
    fn offered_to_claude_and_codex_parents_in_any_mode_never_to_children() {
        assert!(offered(&caller(ProviderKind::Claude, Some("plan")), false));
        assert!(offered(
            &caller(ProviderKind::Claude, Some("default")),
            false
        ));
        assert!(offered(
            &caller(ProviderKind::Codex, Some("read-only")),
            false
        ));
        assert!(offered(
            &caller(ProviderKind::Codex, Some("danger-full-access")),
            false
        ));
        assert!(
            !offered(&caller(ProviderKind::Claude, None), true),
            "running child"
        );
        assert!(!offered(
            &caller(ProviderKind::Cursor, Some("agent")),
            false
        ));
        assert!(!offered(&caller(ProviderKind::OpenCode, None), false));
        let tool = suggest_tool();
        assert_eq!(tool.prefixed_name, "mcp__codemux__suggest_tasks");
        assert!(is_tool(&tool.prefixed_name));
        assert_eq!(tool.input_schema["properties"]["tasks"]["maxItems"], 5);
    }

    #[test]
    fn suggestions_are_validated() {
        let ok = parse_suggestions(&json!({ "tasks": [
            { "title": "  Add   slugify\nhelper ", "task": brief("Add slugify(text)"), "provider": "Codex" },
            { "title": "", "task": format!("Write parser tests\n{}", brief("Cover the parser")) },
        ]}))
        .unwrap();
        assert_eq!(ok[0].title, "Add slugify helper");
        assert_eq!(ok[0].provider, Some(ProviderKind::Codex));
        assert_eq!(ok[1].title, "Write parser tests", "first line of the brief");
        assert_eq!(ok[1].provider, None);

        assert!(parse_suggestions(&json!({ "tasks": [] })).is_err());
        assert!(parse_suggestions(&json!({})).is_err());
        let six: Vec<Value> = (0..6)
            .map(|i| json!({ "title": format!("T{i}"), "task": brief("Do it") }))
            .collect();
        assert!(parse_suggestions(&json!({ "tasks": six }))
            .unwrap_err()
            .contains("at most 5"));
        assert!(
            parse_suggestions(&json!({ "tasks": [{ "title": "x", "task": "too short" }] }))
                .unwrap_err()
                .contains("self-contained")
        );
        assert!(parse_suggestions(
            &json!({ "tasks": [{ "title": "x", "task": brief("Do"), "provider": "hermes" }] })
        )
        .unwrap_err()
        .contains("Unknown agent"));
        let long = parse_suggestions(
            &json!({ "tasks": [{ "title": "t".repeat(90), "task": brief("Do") }] }),
        )
        .unwrap();
        assert_eq!(long[0].title.chars().count(), MAX_TITLE_CHARS);
    }

    #[test]
    fn persisted_calls_are_read_for_both_providers() {
        let args = json!({ "tasks": [{ "title": "A", "task": brief("Do") }] });
        assert_eq!(
            suggest_arguments("mcp__codemux__mcp__codemux__suggest_tasks", &args),
            Some(args.clone())
        );
        let codex =
            json!({ "tool": "codemux_mcp__codemux__suggest_tasks", "arguments": args.to_string() });
        assert_eq!(
            suggest_arguments("dynamicToolCall", &codex),
            Some(args.clone())
        );
        let codex_object =
            json!({ "tool": "codemux_mcp__codemux__suggest_tasks", "arguments": args });
        assert!(suggest_arguments("dynamicToolCall", &codex_object).is_some());
        assert!(suggest_arguments("mcp__other__suggest", &json!({})).is_none());
        assert!(suggest_arguments("mcp__my_codemux__suggest_tasks", &json!({})).is_none());
        assert!(suggest_arguments(
            "dynamicToolCall",
            &json!({ "tool": "codemux_mcp__codemux__delegate_task" })
        )
        .is_none());
    }

    #[test]
    fn branch_names_come_from_the_title_and_are_unique() {
        let existing: HashSet<String> = ["add-slugify-helper".to_string()].into();
        assert_eq!(
            branch_name_for("Add Slugify helper", &existing),
            "add-slugify-helper-2"
        );
        assert_eq!(
            branch_name_for("Write parser tests!", &existing),
            "write-parser-tests"
        );
        assert_eq!(branch_name_for("???", &existing), "follow-up");
    }

    // ── git, in a temp repo with isolated HOME and config ──

    pub(crate) fn task_row(child: &str, parent: &str) -> FollowUpTaskRow {
        FollowUpTaskRow { child_thread_id:child.into(), parent_thread_id:parent.into(), tool_call_id:format!("suggest-{child}"), index:0, workspace_id:String::new(), parent_workspace_id:"workspace".into(), repo_path:String::new(), worktree_path:String::new(), branch:format!("task-{child}"), base_sha:"a".repeat(40), provider:"codex".into(), title:"Review".into(), outcome:"running".into(), additions:None, deletions:None, commits:None, uncommitted:None, merged:false, created_at_ms:1 }
    }

    #[test]
    fn delegation_delivery_suggested_start_after_stop_and_failed_admission_releases_claim() {
        let db = DatabaseStore::new_in_memory();
        let row = task_row("child", "parent");
        let mut task = crate::database::delegation_tasks::TaskRecord::new("suggested".into(), "parent".into(), Some("child".into()), "codex".into(), "Review".into(), "Brief".into(), 1);
        task.admission_generation = Some(db.delivery_generation("parent").unwrap().0);
        db.insert_follow_up_task(&row).unwrap();
        db.stop_message_deliveries("parent").unwrap();
        db.stop_delegation_tree("parent").unwrap();
        assert!(admit_follow_up_intent(&db, &row, &task).is_err());
        assert!(db.get_follow_up_task("child").is_none());
        db.insert_follow_up_task(&row).unwrap();
        task.admission_generation = Some(db.delivery_generation("parent").unwrap().0);
        admit_follow_up_intent(&db, &row, &task).unwrap();
        assert!(db.get_follow_up_task("child").is_some());
        assert!(db.delegation_task("suggested").unwrap().is_some());
    }

    fn repo() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("repo");
        std::fs::create_dir(&path).unwrap();
        git(&path, &["init", "-q", "-b", "main"]).unwrap();
        std::fs::write(path.join("a.txt"), "one\ntwo\nthree\n").unwrap();
        git(&path, &["add", "."]).unwrap();
        git(&path, &["commit", "-q", "-m", "init"]).unwrap();
        (dir, path)
    }

    fn commit_on(repo: &Path, branch: &str, file: &str, body: &str) {
        git(repo, &["checkout", "-q", "-b", branch, "main"]).unwrap();
        std::fs::write(repo.join(file), body).unwrap();
        git(repo, &["add", "."]).unwrap();
        git(repo, &["commit", "-q", "-m", branch]).unwrap();
        git(repo, &["checkout", "-q", "main"]).unwrap();
    }

    fn candidate(branch: &str) -> MergeCandidate {
        MergeCandidate {
            child_thread_id: format!("child-{branch}"),
            title: format!("Task {branch}"),
            branch: branch.into(),
            worktree: None,
        }
    }

    #[test]
    fn base_is_the_parent_head_commit_without_uncommitted_edits() {
        let (_dir, repo) = repo();
        let head = git(&repo, &["rev-parse", "HEAD"]).unwrap();
        std::fs::write(repo.join("a.txt"), "edited but not committed\n").unwrap();
        assert_eq!(head_sha(&repo).unwrap(), head);
        // A branch made from that base does not carry the edit.
        git(&repo, &["branch", "child", &head_sha(&repo).unwrap()]).unwrap();
        assert_eq!(git(&repo, &["rev-parse", "child"]).unwrap(), head);
        assert_eq!(
            git(&repo, &["show", "child:a.txt"]).unwrap(),
            "one\ntwo\nthree"
        );

        let empty = TempDir::new().unwrap();
        git(empty.path(), &["init", "-q"]).unwrap();
        assert!(head_sha(empty.path()).unwrap_err().contains("no commit"));
    }

    #[test]
    fn merge_all_merges_in_order_and_counts_stats() {
        let (_dir, repo) = repo();
        let base = git(&repo, &["rev-parse", "HEAD"]).unwrap();
        commit_on(&repo, "one", "b.txt", "b\n");
        commit_on(&repo, "two", "c.txt", "c\nc\n");
        let result = merge_branches(&repo, &[candidate("one"), candidate("two")]).unwrap();
        assert_eq!(result.target_branch.as_deref(), Some("main"));
        assert_eq!(
            result
                .merged
                .iter()
                .map(|m| m.branch.as_str())
                .collect::<Vec<_>>(),
            vec!["one", "two"]
        );
        assert!(result.conflict.is_none());
        assert!(repo.join("b.txt").exists() && repo.join("c.txt").exists());
        let parents = git(&repo, &["rev-list", "--parents", "-n", "1", "HEAD"]).unwrap();
        assert_eq!(
            parents.split_whitespace().count(),
            3,
            "--no-ff makes a merge commit"
        );

        // Already in: nothing to merge.
        let again = merge_branches(&repo, &[candidate("one")]).unwrap();
        assert!(again.merged.is_empty());
        assert_eq!(again.skipped[0].reason, "No new commits to merge.");

        git(&repo, &["checkout", "-q", "two"]).unwrap();
        assert_eq!(branch_stats(&repo, &base).unwrap(), (2, 0, 1));
    }

    #[test]
    fn a_conflict_is_aborted_reported_and_stops_the_run() {
        let (_dir, repo) = repo();
        commit_on(&repo, "left", "a.txt", "one\nLEFT\nthree\n");
        commit_on(&repo, "right", "a.txt", "one\nRIGHT\nthree\n");
        commit_on(&repo, "later", "d.txt", "d\n");
        let before = git(&repo, &["rev-parse", "HEAD"]).unwrap();
        let result = merge_branches(
            &repo,
            &[candidate("left"), candidate("right"), candidate("later")],
        )
        .unwrap();
        assert_eq!(result.merged.len(), 1, "left merged before the conflict");
        let conflict = result.conflict.expect("right conflicts with left");
        assert_eq!(conflict.branch, "right");
        assert_eq!(conflict.child_thread_id, "child-right");
        assert_eq!(conflict.files, vec!["a.txt".to_string()]);
        assert!(
            !repo.join("d.txt").exists(),
            "nothing after the conflict ran"
        );
        assert!(!repo.join(".git/MERGE_HEAD").exists(), "merge aborted");
        assert!(
            dirty_files(&repo, true).unwrap().is_empty(),
            "clean after abort"
        );
        assert_ne!(
            git(&repo, &["rev-parse", "HEAD"]).unwrap(),
            before,
            "left stays merged"
        );
    }

    #[test]
    fn a_merge_started_between_candidates_stops_the_run_and_keeps_earlier_merges() {
        let (_dir, repo) = repo();
        commit_on(&repo, "one", "b.txt", "b\n");
        commit_on(&repo, "two", "c.txt", "c\n");
        commit_on(&repo, "theirs", "e.txt", "e\n");
        let result = merge_branches_with(&repo, &[candidate("one"), candidate("two")], |next| {
            if next.branch == "two" {
                // Another process starts its own merge right before ours.
                git(&repo, &["merge", "--no-ff", "--no-commit", "theirs"]).unwrap();
            }
        })
        .unwrap();
        assert_eq!(result.merged.len(), 1, "the first merge is kept: {result:?}");
        assert_eq!(result.merged[0].branch, "one");
        assert!(result.conflict.is_none());
        assert!(result.stopped.as_deref().unwrap().contains("another merge"));
        let theirs = git(&repo, &["rev-parse", "theirs"]).unwrap();
        assert_eq!(merge_head(&repo).as_deref(), Some(theirs.as_str()), "their merge is untouched");
        assert!(repo.join("e.txt").exists() && !repo.join("c.txt").exists());
    }

    #[test]
    fn a_failed_merge_over_someone_elses_merge_is_never_theirs_to_abort() {
        let (_dir, repo) = repo();
        commit_on(&repo, "kid", "b.txt", "b\n");
        commit_on(&repo, "theirs", "e.txt", "e\n");
        git(&repo, &["merge", "--no-ff", "--no-commit", "theirs"]).unwrap();
        let kid = git(&repo, &["rev-parse", "kid"]).unwrap();
        // Git refuses to start our merge over theirs.
        let (code, refused) = git_merge(&repo, &["--no-ff", "--no-edit", "kid"]).unwrap_err();
        assert!(!owns_failed_merge(&repo, code, &kid), "{code:?} {refused}");
        assert!(merge_in_progress(&repo), "still in progress");
        // A conflict this run produced is its own.
        git(&repo, &["merge", "--abort"]).unwrap();
        commit_on(&repo, "left", "a.txt", "one\nLEFT\nthree\n");
        commit_on(&repo, "right", "a.txt", "one\nRIGHT\nthree\n");
        git(&repo, &["merge", "--no-ff", "--no-edit", "left"]).unwrap();
        let right = git(&repo, &["rev-parse", "right"]).unwrap();
        let (code, conflict) = git_merge(&repo, &["--no-ff", "--no-edit", "right"]).unwrap_err();
        assert_eq!(code, Some(1), "{conflict}");
        assert!(owns_failed_merge(&repo, code, &right), "{conflict}");
    }

    #[test]
    fn a_conflicting_merge_of_the_same_branch_by_someone_else_keeps_its_resolution_work() {
        let (_dir, repo) = repo();
        commit_on(&repo, "kid", "a.txt", "one\nKID\nthree\n");
        std::fs::write(repo.join("a.txt"), "one\nMAIN\nthree\n").unwrap();
        git(&repo, &["commit", "-q", "-am", "main edit"]).unwrap();
        let kid = git(&repo, &["rev-parse", "kid"]).unwrap();
        let result = merge_branches_with(&repo, &[candidate("kid")], |_| {
            // After our last check, a terminal merges the same branch, hits
            // the conflict and starts resolving it.
            assert!(git(&repo, &["merge", "--no-ff", "--no-edit", "kid"]).is_err());
            std::fs::write(repo.join("a.txt"), "one\nRESOLVED BY HAND\nthree\n").unwrap();
        })
        .unwrap();
        assert!(result.conflict.is_none(), "{result:?}");
        assert!(result.stopped.is_some(), "{result:?}");
        assert!(result.merged.is_empty());
        assert_eq!(merge_head(&repo).as_deref(), Some(kid.as_str()), "their merge is untouched");
        assert_eq!(
            std::fs::read_to_string(repo.join("a.txt")).unwrap(),
            "one\nRESOLVED BY HAND\nthree\n",
            "their resolution edits are kept"
        );
    }

    #[test]
    fn two_merge_all_runs_never_share_a_checkout() {
        let (_dir, repo) = repo();
        let held = MergeLock::acquire(&repo).unwrap();
        let refused = merge_branches(&repo, &[candidate("x")]).unwrap_err();
        assert!(refused.contains("already running"), "{refused}");
        drop(held);
        assert!(MergeLock::acquire(&repo).is_ok());
    }

    #[test]
    fn a_users_merge_in_a_linked_worktree_is_refused_and_left_alone() {
        let (dir, repo) = repo();
        commit_on(&repo, "theirs", "e.txt", "e\n");
        commit_on(&repo, "kid", "b.txt", "b\n");
        let parent = dir.path().join("parent-wt");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "parent",
                parent.to_str().unwrap(),
                "main",
            ],
        )
        .unwrap();
        assert!(parent.join(".git").is_file(), "a linked worktree");
        // The user's own merge, staged but not committed.
        git(&parent, &["merge", "--no-ff", "--no-commit", "theirs"]).unwrap();
        assert!(merge_in_progress(&parent));
        std::fs::create_dir(parent.join("src")).unwrap();
        let sub = parent.join("src");
        assert!(merge_in_progress(&sub), "seen from a subdirectory");

        for target in [&parent, &sub] {
            let refused = merge_branches(target, &[candidate("kid")]).unwrap_err();
            assert!(refused.contains("already in progress"), "{refused}");
        }
        assert!(merge_in_progress(&parent), "the user's merge is untouched");
        assert!(parent.join("e.txt").exists(), "its staged changes too");
        assert!(!parent.join("b.txt").exists(), "nothing was merged");
    }

    #[test]
    fn a_conflict_from_a_subdirectory_of_a_linked_worktree_is_aborted() {
        let (dir, repo) = repo();
        commit_on(&repo, "left", "a.txt", "one\nLEFT\nthree\n");
        let parent = dir.path().join("parent-wt");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "parent",
                parent.to_str().unwrap(),
                "main",
            ],
        )
        .unwrap();
        std::fs::write(parent.join("a.txt"), "one\nPARENT\nthree\n").unwrap();
        std::fs::create_dir(parent.join("src")).unwrap();
        std::fs::write(parent.join("src/keep.txt"), "k\n").unwrap();
        git(&parent, &["add", "."]).unwrap();
        git(&parent, &["commit", "-q", "-m", "parent edit"]).unwrap();
        let before = git(&parent, &["rev-parse", "HEAD"]).unwrap();

        let result = merge_branches(&parent.join("src"), &[candidate("left")]).unwrap();
        let conflict = result.conflict.expect("left conflicts with the parent");
        assert_eq!(conflict.files, vec!["a.txt".to_string()]);
        assert!(!merge_in_progress(&parent), "our own merge was aborted");
        assert_eq!(git(&parent, &["rev-parse", "HEAD"]).unwrap(), before);
        assert!(dirty_files(&parent, true).unwrap().is_empty());
    }

    #[test]
    fn an_interrupted_start_is_settled_on_the_next_launch() {
        let db = DatabaseStore::new_in_memory();
        let row = |child: &str, index: u32, workspace: &str, outcome: &str| FollowUpTaskRow {
            child_thread_id: child.into(),
            parent_thread_id: format!("parent-{child}"),
            tool_call_id: "call".into(),
            index,
            workspace_id: workspace.into(),
            parent_workspace_id: "ws-parent".into(),
            repo_path: "/repo".into(),
            worktree_path: String::new(),
            branch: format!("branch-{child}"),
            base_sha: "a".repeat(40),
            provider: "codex".into(),
            title: format!("Task {child}"),
            outcome: outcome.into(),
            additions: None,
            deletions: None,
            commits: None,
            uncommitted: None,
            merged: false,
            created_at_ms: index as i64,
        };
        // Quit while its worktree was being created: no workspace yet.
        db.insert_follow_up_task(&row("no-ws", 0, "", "running"))
            .unwrap();
        // Quit after the workspace existed but before the journal entry.
        db.insert_follow_up_task(&row("has-ws", 1, "ws-child", "running"))
            .unwrap();
        // Tracked by the delegation state, and being started right now.
        db.insert_follow_up_task(&row("live", 2, "ws-live", "running"))
            .unwrap();
        db.insert_follow_up_task(&row("starting", 3, "", "running"))
            .unwrap();
        db.insert_follow_up_task(&row("done", 4, "ws-done", "completed"))
            .unwrap();
        let _guard = StartingGuard::new("starting");

        let mut parents = reconcile_interrupted_starts(&db, |child| child == "live");
        parents.sort();
        assert_eq!(parents, vec!["parent-has-ws", "parent-no-ws"]);
        assert!(
            db.get_follow_up_task("no-ws").is_none(),
            "the suggestion can be started again"
        );
        db.insert_follow_up_task(&row("retry", 0, "", "running"))
            .expect("its reservation was released");
        assert_eq!(db.get_follow_up_task("has-ws").unwrap().outcome, "stopped");
        assert_eq!(db.get_follow_up_task("live").unwrap().outcome, "running");
        assert_eq!(
            db.get_follow_up_task("starting").unwrap().outcome,
            "running"
        );
        assert_eq!(db.get_follow_up_task("done").unwrap().outcome, "completed");
    }

    #[test]
    fn launch_reconciles_after_draining_the_journal() {
        let app = tauri::test::mock_app();
        app.manage(DatabaseStore::new_in_memory());
        app.manage(DelegationState::default());
        let db = app.state::<DatabaseStore>();
        let row = FollowUpTaskRow {
            child_thread_id: "orphan".into(),
            parent_thread_id: "parent".into(),
            tool_call_id: "call".into(),
            index: 0,
            workspace_id: String::new(),
            parent_workspace_id: "ws-parent".into(),
            repo_path: "/repo".into(),
            worktree_path: String::new(),
            branch: "orphan".into(),
            base_sha: "a".repeat(40),
            provider: "codex".into(),
            title: "Orphan".into(),
            outcome: "running".into(),
            additions: None,
            deletions: None,
            commits: None,
            uncommitted: None,
            merged: false,
            created_at_ms: 1,
        };
        db.insert_follow_up_task(&row).unwrap();
        // The journal is empty: the start never reached it.
        super::super::delegation::drain_journal(app.handle());
        assert!(
            db.get_follow_up_task("orphan").is_none(),
            "the interrupted reservation was released at launch"
        );
    }

    #[test]
    fn a_dirty_parent_is_refused_and_a_dirty_child_is_not_mergeable() {
        let (dir, repo) = repo();
        commit_on(&repo, "kid", "b.txt", "b\n");
        std::fs::write(repo.join("a.txt"), "dirty\n").unwrap();
        let refused = merge_branches(&repo, &[candidate("kid")]).unwrap_err();
        assert!(refused.contains("1 uncommitted change"), "{refused}");
        git(&repo, &["checkout", "--", "a.txt"]).unwrap();

        let worktree = dir.path().join("kid-wt");
        git(
            &repo,
            &["worktree", "add", "-q", worktree.to_str().unwrap(), "kid"],
        )
        .unwrap();
        std::fs::write(worktree.join("left-behind.txt"), "x\n").unwrap();
        let mut kid = candidate("kid");
        kid.worktree = Some(worktree);
        let result = merge_branches(&repo, &[kid]).unwrap();
        assert!(result.merged.is_empty());
        assert!(result.skipped[0]
            .reason
            .starts_with("Not mergeable: 1 uncommitted file"));
    }

    #[test]
    fn worktree_footer_asks_for_a_commit_on_its_branch() {
        let footer = super::super::delegation::WORKTREE_CHILD_FOOTER;
        assert!(footer.contains("COMMIT all of your work on this branch"));
        assert!(footer.contains("uncommitted edits are not here"));
    }

    #[test]
    fn host_lists_and_answers_suggest_tasks() {
        use super::super::delegation::DelegationHost;
        use crate::mcp::registry::HostTools;
        let app = tauri::test::mock_app();
        app.manage(crate::AppMode::ServeHeadless);
        app.manage(DatabaseStore::new_in_memory());
        app.manage(DelegationState::default());
        let host = DelegationHost::with_install_check(app.handle().clone(), |_| true);
        let names = |caller: &HostCaller| {
            host.tools_for(caller)
                .into_iter()
                .map(|tool| tool.name)
                .collect::<Vec<_>>()
        };
        // Codex outside Full access gets suggestions but not delegation.
        assert_eq!(
            names(&caller(ProviderKind::Codex, Some("workspace-write"))),
            vec!["suggest_tasks"]
        );
        assert_eq!(
            names(&caller(ProviderKind::Claude, Some("plan"))),
            vec!["delegate_task", "task_status", "task_cancel", "suggest_tasks"]
        );
        assert!(names(&caller(ProviderKind::Cursor, None)).is_empty());

        let claude = caller(ProviderKind::Claude, Some("default"));
        let answer = host
            .call(
                &claude,
                "mcp__codemux__suggest_tasks",
                &json!({ "tasks": [{ "title": "Add slugify helper", "task": brief("Add slugify(text)") }] }),
            )
            .unwrap();
        assert_eq!(answer["isError"], false);
        let text = answer["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("\"suggested\":1") && text.contains("Nothing has started"));
        let refused = host
            .call(
                &claude,
                "mcp__codemux__suggest_tasks",
                &json!({ "tasks": [] }),
            )
            .unwrap();
        assert_eq!(refused["isError"], true);

        // A running child is neither offered nor served the tool.
        let state = app.state::<DelegationState>();
        state.reserve_worktree(
            "parent",
            Task::new("grandparent", ProviderKind::Codex, "t", "b", 0),
        );
        assert!(names(&claude).is_empty());
        let refused = host
            .call(
                &claude,
                "mcp__codemux__suggest_tasks",
                &json!({ "tasks": [{ "title": "x", "task": brief("Do") }] }),
            )
            .unwrap();
        assert_eq!(refused["isError"], true);
    }
}
