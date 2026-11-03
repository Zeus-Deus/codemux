//! Who is calling over the control socket: client names, requested access
//! and whether the process runs under an agent Codemux started.
//!
//! Everything a client says about itself is self-asserted. A name attributes
//! and an access level narrows; neither isolates same-user processes from
//! each other.

use crate::agent_control::{Access, CallerView, ClientName, ControlAccess, ControlCaller, ControlError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::OnceLock;

/// What a requesting process says about itself on the control socket.
///
/// No `deny_unknown_fields`: a newer CLI may add fields an older app ignores.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ControlClient {
    /// Stable client label (`--client`, or MCP `clientInfo.name`). Becomes `local:<slug>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Requested ceiling. It can only narrow what the socket would otherwise grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<ControlAccess>,
}

// One process has one control identity: the CLI and `codemux mcp` set it once
// and every socket call from the process picks it up.
static CLIENT_NAME: OnceLock<String> = OnceLock::new();
static CLIENT_ACCESS: OnceLock<ControlAccess> = OnceLock::new();

/// Names this process's client. The first usable name wins, so `--client`
/// (set at startup) beats a later MCP `clientInfo.name`. Returns whether this
/// call set it.
pub fn set_client_name(raw: &str) -> bool {
    adopt_name(&CLIENT_NAME, raw)
}

fn adopt_name(slot: &OnceLock<String>, raw: &str) -> bool {
    let raw = raw.trim();
    // A name with nothing left of its slug ("---", "") would resolve to the
    // owner on the server, so it does not claim the slot either.
    ClientName::parse(raw).is_some() && slot.set(raw.to_owned()).is_ok()
}

/// Narrows this process's access. The first call wins.
pub fn set_client_access(access: ControlAccess) {
    let _ = CLIENT_ACCESS.set(access);
}

impl ControlClient {
    /// This process's identity, or `None` when it has nothing to say (the owner).
    pub fn current() -> Option<Self> {
        Self::from_parts(CLIENT_NAME.get().map(String::as_str), CLIENT_ACCESS.get().copied())
    }

    /// Pure core of [`current`](Self::current), for tests.
    pub(crate) fn from_parts(name: Option<&str>, access: Option<ControlAccess>) -> Option<Self> {
        let client = Self { name: name.map(str::to_owned), access };
        (client != Self::default()).then_some(client)
    }

    /// Whether this client asks for less than full access. Such a client must
    /// not talk to a server that would ignore the request (`server_too_old`).
    pub(crate) fn narrows(&self) -> bool {
        matches!(self.access, Some(ControlAccess::ReadOnly | ControlAccess::Supervised))
    }
}

// ---------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------

/// Declares each socket command's access class once. `CLASSIFIED` and the
/// lookup are built from the same list, so a command cannot be named in one
/// and missing from the other.
macro_rules! socket_commands {
    ($($access:ident => [$($name:literal),+ $(,)?]),+ $(,)?) => {
        /// Every control-socket command with a declared access class. A
        /// source-scan test checks that each `control.rs` dispatch arm is here.
        #[cfg(test)]
        pub(crate) const CLASSIFIED: &[&str] = &[$($($name),+),+];

        fn declared_access(command: &str) -> Option<Access> {
            match command {
                $($($name)|+ => Some(Access::$access),)+
                _ => None,
            }
        }
    };
}

socket_commands! {
    Read => [
        "status", "get_app_state", "conversation_read", "conversation_search",
        "agent_chat_scan_local_sessions", "get_presets", "read_terminal", "list_archived_workspaces",
        "monitor_status", "port_list", "get_project_memory", "generate_handoff", "index_status",
        "search_index", "list_github_issues", "get_github_issue", "automation_list", "automation_get",
        "automation_runs", "web_remote_status", "delegation_local_context", "delegation_host_info",
        "delegation_grants", "delegation_list", "delegation_read",
        // Registry routes: the registry applies each tool's own access class.
        "tool_call", "tools_list",
    ],
    Write => [
        "agent_chat_import_local_sessions", "split_pane", "create_browser_pane", "open_url", "notify",
        "activate_workspace", "close_workspace", "archive_workspace", "close_pane", "monitor_start",
        "monitor_stop", "browser_automation", "update_project_memory", "add_project_memory_entry",
        "rebuild_index", "link_workspace_issue", "reload_ui", "delegate_task", "delegation_cancel",
        "delegation_deliver",
    ],
    // Runs repository setup scripts or removes files.
    Machine => ["create_workspace", "create_worktree_workspace", "unarchive_workspace", "rerun_setup"],
    // Changes who can reach or control Codemux, schedules unattended work,
    // answers on the person's behalf, or runs arbitrary input as the owner.
    Admin => [
        "apply_preset", "write_terminal", "automation_create", "automation_update",
        "automation_set_enabled", "automation_delete", "web_remote_pair", "web_remote_enable",
        "web_remote_set_relay", "web_remote_disable", "agent_connector_status",
        "agent_connector_set_config", "agent_connector_approve", "agent_connector_deny",
        "agent_connector_revoke", "agent_connector_issue", "agent_connector_revoke_all",
        "delegation_authorize", "delegation_revoke", "delegation_respond",
    ],
}

/// The access a socket command needs. Unknown commands are `Admin`, so they
/// fail closed for everyone but the owner (who then gets "Unknown control
/// command").
pub fn command_access(command: &str, params: &Value) -> Access {
    // The native tool aliases are registry tools; the registry gates them
    // with each tool's own class and the caller's access.
    if crate::agent_control::is_native_tool(command) {
        return Access::Read;
    }
    match command {
        "browser_automation" => browser_action_access(params.get("action").unwrap_or(&Value::Null)),
        "create_worktree_workspace" if launches_an_agent(params) => Access::Admin,
        "close_workspace" if removes_files(params) => Access::Machine,
        _ => declared_access(command).unwrap_or(Access::Admin),
    }
}

/// The access a legacy stdio MCP tool needs, from the socket command it
/// becomes (or, for tools that run inside `codemux mcp`, from what they do).
/// `None` for names that are not legacy tools.
pub fn mcp_tool_access(tool: &str, arguments: &Value) -> Option<Access> {
    Some(match tool {
        "browser_snapshot" | "browser_accessibility_snapshot" | "browser_screenshot" | "browser_console_logs"
        | "browser_get_styles" | "browser_wait" | "browser_viewport_presets" | "workspace_list"
        | "workspace_info" | "pane_list" | "app_status" | "workspace_archive_list" | "automation_list"
        | "automation_get" | "automation_runs" | "conversation_read" | "conversation_search" | "git_status"
        | "git_diff" | "terminal_read" | "port_list" | "preset_list" | "issue_list" | "issue_get"
        | "html_render" | "delegation_targets" | "task_status" => Access::Read,
        "workspace_close" if removes_files(arguments) => Access::Machine,
        "browser_navigate" | "browser_click" | "browser_fill" | "browser_click_at" | "browser_type_at"
        | "browser_scroll_at" | "browser_key_press" | "browser_drag" | "browser_click_os" | "browser_type_os"
        | "browser_evaluate" | "browser_viewport" | "workspace_open" | "workspace_close" | "workspace_archive"
        | "pane_split_right" | "pane_split_down" | "pane_close" | "notify" | "git_stage" | "git_commit"
        | "git_push" | "issue_link_workspace" | "delegate_remote_task" | "task_cancel" => Access::Write,
        "worktree_create" if launches_an_agent(arguments) => Access::Admin,
        "workspace_create" | "worktree_create" | "workspace_unarchive" => Access::Machine,
        "automation_create" | "automation_update" | "automation_delete" | "automation_pause"
        | "automation_resume" | "terminal_write" | "preset_apply" => Access::Admin,
        _ => return None,
    })
}

/// The stdio-side gate for a narrowed `codemux mcp`. It matters for the
/// tools that never reach the socket (local git, `html_render`); the socket
/// stays authoritative for everything else.
pub fn stdio_permits(tool: &str, arguments: &Value, access: ControlAccess) -> Result<(), ControlError> {
    match mcp_tool_access(tool, arguments) {
        Some(required) => narrowed(tool, required, access),
        None => Ok(()),
    }
}

/// `browser_automation` reads only for actions that inspect the page,
/// including the DOM snapshot that `browser_snapshot` runs as an `eval`.
fn browser_action_access(action: &Value) -> Access {
    // The handler's own default action is `open_url`.
    let kind = action.get("kind").and_then(Value::as_str).unwrap_or("open_url");
    let reads = matches!(
        kind,
        "snapshot" | "accessibility_snapshot" | "screenshot" | "console" | "console_logs" | "get_styles"
            | "wait" | "get_text" | "get_box"
    ) || (kind == "eval"
        && action.get("script").and_then(Value::as_str) == Some(crate::agent_browser::DOM_SNAPSHOT_SCRIPT));
    if reads {
        Access::Read
    } else {
        Access::Write
    }
}

/// A worktree that starts a preset CLI agent, or types a prompt into one, is
/// `apply_preset` by another name.
fn launches_an_agent(params: &Value) -> bool {
    ["agent_preset_id", "initial_prompt"].iter().any(|key| params.get(key).is_some_and(|value| !value.is_null()))
}

fn removes_files(params: &Value) -> bool {
    ["delete_worktree", "delete_branch", "force_delete"]
        .iter()
        .any(|key| params.get(key).and_then(Value::as_bool) == Some(true))
}

/// The gate for a caller whose only limit is the access it asked for.
fn narrowed(command: &str, required: Access, access: ControlAccess) -> Result<(), ControlError> {
    required.check(&CallerView::outside(access)).map_err(|_| {
        let needs = if required.minimum() == ControlAccess::FullAccess { "full access" } else { "write access" };
        let connection = if access == ControlAccess::ReadOnly { "read-only" } else { "supervised" };
        ControlError::new(ErrorCode::AccessDenied, format!("This connection is {connection}; `{command}` needs {needs}"))
    })
}

// ---------------------------------------------------------------------------
// Server side
// ---------------------------------------------------------------------------

/// One socket request's caller, as far as the socket can establish it.
#[derive(Debug, Clone)]
pub(crate) enum SocketCaller {
    /// The owner (principal `local`) or a named same-user client (`local:<slug>`).
    Local(ControlCaller),
}

impl SocketCaller {
    pub(crate) fn owner() -> Self {
        Self::Local(ControlCaller::trusted())
    }

    /// The gate every socket command passes before its handler runs.
    pub(crate) fn permits(&self, command: &str, required: Access) -> Result<(), ControlError> {
        match self {
            // The owner's gate (`Access::check` with `CallerKind::Owner`): an
            // unnamed or named local caller is limited only by the access it
            // asked for.
            Self::Local(caller) => narrowed(command, required, caller.access),
        }
    }
}

/// Resolves who sent a socket request. No client is the owner; a named client
/// is `local:<slug>`, attributed as an outside assistant.
pub(crate) fn resolve_socket_caller(client: Option<ControlClient>) -> SocketCaller {
    let client = client.unwrap_or_default();
    let access = client.access.unwrap_or(ControlAccess::FullAccess);
    let name = client.name.as_deref().and_then(ClientName::parse);
    SocketCaller::Local(ControlCaller::local(name.as_ref(), access))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use ControlAccess::{FullAccess, ReadOnly, Supervised};

    #[test]
    fn control_client_from_parts_names_and_narrows() {
        assert_eq!(ControlClient::from_parts(None, None), None, "the owner sends nothing");
        let named = ControlClient::from_parts(Some("Hermes Agent"), None).unwrap();
        assert_eq!((named.name.as_deref(), named.access), (Some("Hermes Agent"), None));
        assert!(!named.narrows());
        let narrowed = ControlClient::from_parts(None, Some(Supervised)).unwrap();
        assert!(narrowed.narrows());
        assert!(ControlClient::from_parts(None, Some(ReadOnly)).unwrap().narrows());
        assert!(!ControlClient::from_parts(None, Some(FullAccess)).unwrap().narrows());
    }

    #[test]
    fn first_usable_client_name_wins() {
        let slot = OnceLock::new();
        assert!(!adopt_name(&slot, "  "), "blank names never claim the slot");
        assert!(!adopt_name(&slot, "---"), "a name with an empty slug never claims the slot");
        assert!(adopt_name(&slot, " hermes "), "the --client flag is set first");
        assert!(!adopt_name(&slot, "Claude Code"), "MCP clientInfo arrives later and loses");
        assert_eq!(slot.get().map(String::as_str), Some("hermes"));
    }

    #[test]
    fn every_socket_command_is_classified() {
        let source = include_str!("control.rs");
        let start = source.find("fn dispatch_request_as").expect("dispatch_request_as exists");
        let end = start + source[start..].find("_ => Err(format!(\"Unknown control command").expect("unknown arm exists");
        let mut arms = Vec::new();
        for line in source[start..end].lines() {
            let Some(rest) = line.strip_prefix("        \"") else { continue };
            let pattern = &rest[..rest.find("=>").unwrap_or(rest.len())];
            let pattern = format!("\"{pattern}");
            for literal in pattern.split('|') {
                let literal = literal.trim();
                assert!(literal.starts_with('"') && literal.ends_with('"'), "unexpected arm pattern {literal:?}");
                arms.push(literal.trim_matches('"').to_owned());
            }
        }
        assert!(arms.len() > 60, "the scan found the dispatch arms ({})", arms.len());
        let unclassified: Vec<_> = arms.iter().filter(|arm| !CLASSIFIED.contains(&arm.as_str())).collect();
        assert!(unclassified.is_empty(), "socket commands without an access class: {unclassified:?}");
        let mut unique = CLASSIFIED.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), CLASSIFIED.len(), "a command is classified twice");
    }

    #[test]
    fn every_legacy_mcp_tool_is_classified() {
        let missing: Vec<_> = crate::mcp_server::LEGACY_TOOL_NAMES
            .iter()
            .filter(|tool| mcp_tool_access(tool, &json!({})).is_none())
            .collect();
        assert!(missing.is_empty(), "legacy tools without an access class: {missing:?}");
        assert_eq!(mcp_tool_access("thread_launch", &json!({})), None, "registry tools are gated by the registry");
        for (tool, access) in [
            ("terminal_write", Access::Admin), ("preset_apply", Access::Admin), ("automation_pause", Access::Admin),
            ("workspace_create", Access::Machine), ("workspace_unarchive", Access::Machine),
            ("git_push", Access::Write), ("git_status", Access::Read), ("workspace_info", Access::Read),
        ] {
            assert_eq!(mcp_tool_access(tool, &json!({})), Some(access), "{tool}");
        }
        assert_eq!(mcp_tool_access("workspace_close", &json!({})), Some(Access::Write));
        assert_eq!(mcp_tool_access("workspace_close", &json!({"delete_worktree": true})), Some(Access::Machine));
        assert_eq!(mcp_tool_access("worktree_create", &json!({"initial_prompt": "go"})), Some(Access::Admin));
    }

    #[test]
    fn admin_commands_match_the_brief() {
        for command in [
            "agent_connector_status", "agent_connector_set_config", "agent_connector_approve",
            "agent_connector_deny", "agent_connector_revoke", "agent_connector_issue",
            "agent_connector_revoke_all", "web_remote_enable", "web_remote_pair", "web_remote_set_relay",
            "web_remote_disable", "automation_create", "automation_update", "automation_set_enabled",
            "automation_delete", "write_terminal", "apply_preset", "delegation_authorize",
            "delegation_revoke", "delegation_respond", "nope",
        ] {
            assert_eq!(command_access(command, &json!({})), Access::Admin, "{command}");
        }
        for (command, access) in [
            ("status", Access::Read), ("web_remote_status", Access::Read), ("tool_call", Access::Read),
            ("tools_list", Access::Read), ("thread_list", Access::Read), ("thread_launch", Access::Read),
            ("notify", Access::Write), ("create_workspace", Access::Machine), ("rerun_setup", Access::Machine),
            ("unarchive_workspace", Access::Machine), ("close_workspace", Access::Write),
        ] {
            assert_eq!(command_access(command, &json!({})), access, "{command}");
        }
        assert_eq!(command_access("close_workspace", &json!({"force_delete": true})), Access::Machine);
        assert_eq!(command_access("close_workspace", &json!({"delete_worktree": false})), Access::Write);
    }

    #[test]
    fn browser_automation_class_follows_the_action() {
        let access = |action: Value| command_access("browser_automation", &json!({ "action": action }));
        for kind in ["snapshot", "screenshot", "console", "get_styles", "wait"] {
            assert_eq!(access(json!({ "kind": kind })), Access::Read, "{kind}");
        }
        let dom = json!({ "kind": "eval", "script": crate::agent_browser::DOM_SNAPSHOT_SCRIPT });
        assert_eq!(access(dom), Access::Read, "browser_snapshot's DOM eval reads");
        assert_eq!(access(json!({ "kind": "eval", "script": "document.body.remove()" })), Access::Write);
        assert_eq!(access(json!({ "kind": "open", "url": "about:blank" })), Access::Write);
        assert_eq!(access(json!({ "kind": "click_os" })), Access::Write);
        assert_eq!(access(json!({})), Access::Write, "a missing kind is the handler's default open_url");
        assert_eq!(command_access("browser_automation", &json!({})), Access::Write);
    }

    #[test]
    fn worktree_with_a_preset_is_admin() {
        let access = |params: Value| command_access("create_worktree_workspace", &params);
        assert_eq!(access(json!({ "repo_path": "/r", "branch": "b", "agent_preset_id": "claude" })), Access::Admin);
        assert_eq!(access(json!({ "repo_path": "/r", "branch": "b", "initial_prompt": "fix it" })), Access::Admin);
        assert_eq!(access(json!({ "repo_path": "/r", "branch": "b" })), Access::Machine);
        assert_eq!(
            access(json!({ "repo_path": "/r", "branch": "b", "agent_preset_id": null, "initial_prompt": null })),
            Access::Machine,
            "null is absent"
        );
    }

    #[test]
    fn local_caller_permission_matrix() {
        let named = ClientName::parse("hermes").unwrap();
        let owner = SocketCaller::owner();
        let supervised = SocketCaller::Local(ControlCaller::local(Some(&named), Supervised));
        let read_only = SocketCaller::Local(ControlCaller::local(Some(&named), ReadOnly));
        // (caller, the classes it may run)
        let table: [(&SocketCaller, [bool; 5]); 3] = [
            (&owner, [true, true, true, true, true]),
            (&supervised, [true, true, true, false, false]),
            (&read_only, [true, false, false, false, false]),
        ];
        let classes = [Access::Read, Access::Write, Access::StartsThreads, Access::Machine, Access::Admin];
        for (caller, allowed) in table {
            for (class, allowed) in classes.iter().zip(allowed) {
                let result = caller.permits("cmd", *class);
                assert_eq!(result.is_ok(), allowed, "{caller:?} {class:?}");
                if let Err(error) = result {
                    assert_eq!(error.code, ErrorCode::AccessDenied);
                }
            }
        }
        let refused = read_only.permits("notify", Access::Write).unwrap_err();
        assert_eq!(refused.to_string(), "access_denied: This connection is read-only; `notify` needs write access");
        let refused = supervised.permits("create_workspace", Access::Machine).unwrap_err();
        assert_eq!(refused.to_string(), "access_denied: This connection is supervised; `create_workspace` needs full access");
    }

    #[test]
    fn socket_clients_resolve_to_local_principals() {
        let SocketCaller::Local(owner) = resolve_socket_caller(None);
        assert_eq!(owner.principal, "local");
        assert_eq!(owner.access, FullAccess);
        let SocketCaller::Local(narrowed) = resolve_socket_caller(Some(ControlClient { name: None, access: Some(ReadOnly) }));
        assert_eq!((narrowed.principal.as_str(), narrowed.access), ("local", ReadOnly));
        let SocketCaller::Local(named) =
            resolve_socket_caller(Some(ControlClient { name: Some("Hermes Agent".into()), access: Some(Supervised) }));
        assert_eq!(named.principal, "local:hermes-agent");
        assert_eq!(named.label.as_deref(), Some("Hermes Agent"));
        assert_eq!(named.access, Supervised);
        let SocketCaller::Local(junk) = resolve_socket_caller(Some(ControlClient { name: Some("---".into()), access: None }));
        assert_eq!(junk.principal, "local", "an unusable name is the owner");
    }

    #[test]
    fn stdio_gate_refuses_local_git_writes_to_read_only_clients() {
        let refused = stdio_permits("git_commit", &json!({ "message": "x" }), ReadOnly).unwrap_err();
        assert_eq!(refused.code, ErrorCode::AccessDenied);
        assert!(stdio_permits("git_status", &json!({}), ReadOnly).is_ok());
        assert!(stdio_permits("git_commit", &json!({}), Supervised).is_ok());
        assert_eq!(stdio_permits("terminal_write", &json!({}), Supervised).unwrap_err().code, ErrorCode::AccessDenied);
        assert!(stdio_permits("thread_launch", &json!({}), ReadOnly).is_ok(), "the socket gates registry tools");
    }
}
