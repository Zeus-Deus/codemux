//! `codemux workspace …` and `codemux worktree create`: open, list, inspect
//! and close workspaces, and create git worktrees as workspaces.

use super::args::{self, OutputArgs};
use super::door::{self, Door};
use super::output::{self, CliError, Exit};
use super::verbs;
use crate::control::{send_control_request, ControlRequest};
use clap::Subcommand;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::time::Instant;

#[derive(Subcommand)]
pub enum WorkspaceCommand {
    /// Open a folder as a workspace (or find the one already open there) and
    /// print its id. Never takes focus unless --select.
    Create {
        /// The folder to open.
        path: PathBuf,
        /// Focus the workspace in Codemux.
        #[arg(long)]
        select: bool,
        /// Idempotency key (default: a new one).
        #[arg(long, value_name = "ID")]
        request_id: Option<String>,
        #[command(flatten)]
        out: OutputArgs,
    },
    /// List open workspaces.
    List {
        #[command(flatten)]
        out: OutputArgs,
    },
    /// Show one workspace (default $CODEMUX_WORKSPACE_ID, else the one
    /// focused in Codemux).
    Info {
        /// Workspace id.
        id: Option<String>,
        #[command(flatten)]
        out: OutputArgs,
    },
    /// Close a workspace. Its files and branch stay unless asked.
    Close {
        /// Workspace id.
        id: String,
        /// Also remove its git worktree from disk.
        #[arg(long)]
        remove_worktree: bool,
        /// Also delete the worktree's branch.
        #[arg(long, requires = "remove_worktree")]
        delete_branch: bool,
        /// Remove the worktree even with uncommitted or unpushed work.
        #[arg(long, requires = "remove_worktree")]
        force: bool,
        /// Idempotency key (default: a new one).
        #[arg(long, value_name = "ID")]
        request_id: Option<String>,
        #[command(flatten)]
        out: OutputArgs,
    },
    /// Re-run setup scripts (.codemuxinclude + setup commands) for a workspace
    RerunSetup {
        /// Workspace ID (defaults to active workspace)
        workspace_id: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum WorktreeCommand {
    /// Create a git worktree on a branch, open it as a workspace and print
    /// its id. Never takes focus.
    Create {
        /// The repository folder.
        #[arg(long, value_name = "PATH")]
        repo: PathBuf,
        /// The worktree's branch. New unless --existing.
        #[arg(long, value_name = "BRANCH")]
        branch: String,
        /// Where the new branch starts (default: the repository's default branch).
        #[arg(long, value_name = "REF", conflicts_with = "existing")]
        base: Option<String>,
        /// Check out an existing branch instead of creating one.
        #[arg(long)]
        existing: bool,
        /// Idempotency key (default: a new one).
        #[arg(long, value_name = "ID")]
        request_id: Option<String>,
        #[command(flatten)]
        out: OutputArgs,
    },
}

/// Workspace tools run setup and teardown scripts; their receipts settle
/// within this.
const SETTLE_BUDGET: Duration = Duration::from_secs(120);

pub(crate) async fn run(door: &Door, command: WorkspaceCommand) -> Result<Exit, String> {
    match command {
        WorkspaceCommand::Create { path, select, request_id, out } => {
            let key = args::request_id(request_id.as_deref());
            let outcome = async {
                let mut arguments = json!({"path": absolute(&path)?, "client_request_id": key});
                if select {
                    arguments["select"] = json!(true);
                }
                mutate(door, verbs::WORKSPACE_CREATE, arguments).await
            }
            .await;
            output::report(verbs::WORKSPACE_CREATE, outcome, out.json, Some(&key), |opened| {
                let id = workspace_id(opened);
                output::out(id)?;
                let cwd = opened["cwd"].as_str().map_or_else(|| path.display().to_string(), str::to_owned);
                output::note(&if opened["created"] == false {
                    format!("{cwd} is already open as {id}.")
                } else {
                    format!("Opened {cwd} as {id}.")
                });
                Ok(())
            })
        }
        WorkspaceCommand::List { out } => {
            let outcome = door.call(verbs::WORKSPACE_LIST, json!({})).await;
            output::report(verbs::WORKSPACE_LIST, outcome, out.json, None, |listed| {
                let mut rows = vec![vec!["ID".to_owned(), "BRANCH".to_owned(), "CWD".to_owned(), "TITLE".to_owned()]];
                for workspace in listed["workspaces"].as_array().into_iter().flatten() {
                    let cell = |key: &str| workspace[key].as_str().unwrap_or_default().to_owned();
                    rows.push(vec![cell("workspace_id"), cell("branch"), cell("cwd"), cell("title")]);
                }
                output::table(&rows)
            })
        }
        WorkspaceCommand::Info { id, out } => {
            // Without an id or $CODEMUX_WORKSPACE_ID the app answers with
            // the focused workspace.
            let arguments = match id.or_else(|| std::env::var("CODEMUX_WORKSPACE_ID").ok()).filter(|id| !id.is_empty()) {
                Some(id) => json!({"workspace_id": id}),
                None => json!({}),
            };
            let outcome = door.call(verbs::WORKSPACE_INFO, arguments).await;
            output::report(verbs::WORKSPACE_INFO, outcome, out.json, None, |snapshot| {
                for (key, value) in snapshot.as_object().into_iter().flatten() {
                    let shown = match value {
                        Value::String(text) => text.clone(),
                        Value::Array(items) => format!("{} items", items.len()),
                        Value::Object(_) => continue,
                        other => other.to_string(),
                    };
                    output::out(&format!("{key}: {shown}"))?;
                }
                Ok(())
            })
        }
        WorkspaceCommand::Close { id, remove_worktree, delete_branch, force, request_id, out } => {
            let key = args::request_id(request_id.as_deref());
            let mut arguments = json!({"workspace_id": id, "client_request_id": key});
            for (flag, given) in [("delete_worktree", remove_worktree), ("delete_branch", delete_branch), ("force_delete", force)] {
                if given {
                    arguments[flag] = json!(true);
                }
            }
            let outcome = mutate(door, verbs::WORKSPACE_CLOSE, arguments).await;
            output::report(verbs::WORKSPACE_CLOSE, outcome, out.json, Some(&key), |closed| {
                let removed = closed["worktree_removed"] == true || closed["result"]["worktree_removed"] == true;
                output::out(&format!("Closed {}{}.", workspace_id(closed), if removed { " and removed its worktree" } else { "" }))
            })
        }
        WorkspaceCommand::RerunSetup { workspace_id } => {
            let mut params = json!({});
            if let Some(id) = workspace_id {
                params["workspace_id"] = json!(id);
            } else if let Ok(id) = std::env::var("CODEMUX_WORKSPACE_ID") {
                params["workspace_id"] = json!(id);
            }
            let response = send_control_request(ControlRequest { command: "rerun_setup".into(), params }).await?;
            super::print_response(&response)
        }
    }
}

pub(crate) async fn run_worktree(door: &Door, command: WorktreeCommand) -> Result<Exit, String> {
    let WorktreeCommand::Create { repo, branch, base, existing, request_id, out } = command;
    let key = args::request_id(request_id.as_deref());
    let outcome = async {
        let mut arguments = json!({"repo_path": absolute(&repo)?, "branch": branch, "client_request_id": key});
        if let Some(base) = base {
            arguments["base"] = json!(base);
        }
        if existing {
            arguments["new_branch"] = json!(false);
        }
        mutate(door, verbs::WORKTREE_CREATE, arguments).await
    }
    .await;
    output::report(verbs::WORKTREE_CREATE, outcome, out.json, Some(&key), |created| {
        let id = workspace_id(created);
        output::out(id)?;
        let cwd = created["cwd"].as_str().or_else(|| created["result"]["cwd"].as_str()).unwrap_or("the worktree");
        output::note(&if created["adopted"] == true {
            format!("Opened the existing checkout {cwd} as {id}.")
        } else {
            format!("Created {cwd} as {id}.")
        });
        Ok(())
    })
}

/// The workspace a result names: the tool's own result, or a replayed
/// receipt's.
fn workspace_id(value: &Value) -> &str {
    value["workspace_id"].as_str().or_else(|| value["result"]["workspace_id"].as_str()).unwrap_or_default()
}

async fn mutate(door: &Door, tool: &str, arguments: Value) -> Result<Value, CliError> {
    let created = door.call(tool, arguments).await?;
    door::settle(door, created, Instant::now() + SETTLE_BUDGET).await
}

/// Paths go to the app as absolute paths, resolved against this shell's
/// directory rather than the app's.
fn absolute(path: &Path) -> Result<String, CliError> {
    std::path::absolute(path)
        .map(|path| path.display().to_string())
        .map_err(|error| CliError::failed(door::code::INVALID_ARGUMENTS, format!("{}: {error}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, CommandSet};
    use clap::Parser;

    fn workspace(argv: &[&str]) -> WorkspaceCommand {
        match Cli::try_parse_from([&["codemux", "workspace"][..], argv].concat()).expect("valid argv").command {
            Some(CommandSet::Workspace { command }) => command,
            _ => panic!("expected workspace"),
        }
    }

    #[tokio::test]
    async fn close_flags_map_to_the_tool_and_only_when_given() {
        let closed = || Ok(json!({"workspace_id": "ws", "closed": true, "worktree_removed": true, "operation_id": "op", "state": "succeeded"}));
        let door = Door::fake(vec![("workspace_close", closed()), ("workspace_close", closed())]);
        run(&door, workspace(&["close", "ws", "--remove-worktree", "--delete-branch", "--force", "--request-id", "k", "--json"])).await.unwrap();
        run(&door, workspace(&["close", "ws", "--request-id", "k2"])).await.unwrap();
        let calls = door.script().calls();
        assert_eq!(
            calls[0].1,
            json!({"workspace_id": "ws", "delete_worktree": true, "delete_branch": true, "force_delete": true, "client_request_id": "k"})
        );
        assert_eq!(calls[1].1, json!({"workspace_id": "ws", "client_request_id": "k2"}));
    }

    #[tokio::test]
    async fn create_sends_an_absolute_path_and_select_only_when_asked() {
        let opened = || Ok(json!({"workspace_id": "ws", "cwd": "/x", "created": true, "operation_id": "op", "state": "succeeded"}));
        let door = Door::fake(vec![("workspace_create", opened()), ("workspace_create", opened())]);
        assert_eq!(run(&door, workspace(&["create", "rel/dir", "--request-id", "k"])).await.unwrap(), Exit::Done);
        assert_eq!(run(&door, workspace(&["create", "/abs", "--select", "--request-id", "k"])).await.unwrap(), Exit::Done);
        let calls = door.script().calls();
        let path = calls[0].1["path"].as_str().unwrap();
        assert!(Path::new(path).is_absolute() && path.ends_with("rel/dir"), "{path}");
        assert!(calls[0].1.get("select").is_none());
        assert_eq!(calls[1].1, json!({"path": "/abs", "select": true, "client_request_id": "k"}));
    }

    #[tokio::test]
    async fn worktree_create_checks_out_an_existing_branch_on_request() {
        let created = || Ok(json!({"workspace_id": "ws", "cwd": "/w", "adopted": false, "base_branch": "trunk", "operation_id": "op", "state": "succeeded"}));
        let door = Door::fake(vec![("worktree_create", created()), ("worktree_create", created())]);
        let worktree = |argv: &[&str]| match Cli::try_parse_from([&["codemux", "worktree", "create"][..], argv].concat()).unwrap().command {
            Some(CommandSet::Worktree { command }) => command,
            _ => panic!("expected worktree"),
        };
        run_worktree(&door, worktree(&["--repo", "/r", "--branch", "b", "--base", "trunk", "--request-id", "k"])).await.unwrap();
        run_worktree(&door, worktree(&["--repo", "/r", "--branch", "b", "--existing", "--request-id", "k"])).await.unwrap();
        let calls = door.script().calls();
        assert_eq!(calls[0].1, json!({"repo_path": "/r", "branch": "b", "base": "trunk", "client_request_id": "k"}));
        assert_eq!(calls[1].1, json!({"repo_path": "/r", "branch": "b", "new_branch": false, "client_request_id": "k"}));
    }

    #[tokio::test]
    async fn a_failed_receipt_exits_1() {
        let door = Door::fake(vec![(
            "workspace_close",
            Ok(json!({"operation_id": "op", "state": "failed", "error": {"code": "workspace_close_refused", "message": "Use force to override."}})),
        )]);
        assert_eq!(run(&door, workspace(&["close", "ws", "--json"])).await.unwrap(), Exit::Failed);
    }
}
