//! Who is calling over the control socket: client names, requested access
//! and whether the process runs under an agent Codemux started.
//!
//! Everything a client says about itself is self-asserted. A name attributes
//! and an access level narrows; neither isolates same-user processes from
//! each other.

use crate::agent_control::{Access, CallerView, ClientName, ControlAccess, ControlCaller, ControlError, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::ffi::OsString;
use std::sync::OnceLock;

/// Present (any value, even empty) on every process Codemux starts for an
/// agent, and inherited by everything that process starts.
pub const AGENT_PROCESS_ENV: &str = "CODEMUX_AGENT_PROCESS";

/// Env that carries an agent's identity. An app or `codemux serve` instance
/// drops it at startup and no user terminal ever inherits it.
pub const AGENT_IDENTITY_ENV_VARS: [&str; 4] = [
    AGENT_PROCESS_ENV,
    crate::mcp::gateway::BRIDGE_URL_ENV,
    crate::mcp::gateway::BRIDGE_TOKEN_ENV,
    crate::mcp_server::PROVIDER_ORIGIN_ENV,
];

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
    /// Present when the process runs under an agent that Codemux started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentProcess>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentProcess {
    /// What started it (`1` for provider runtimes, `opencode`, `automation`,
    /// `resolver`, `mcp`, ...). Diagnostic only, never used to authorize; at
    /// most 32 bytes.
    pub kind: String,
}

impl AgentProcess {
    const MAX_KIND: usize = 32;

    fn from_marker(value: &str) -> Self {
        let mut end = value.len().min(Self::MAX_KIND);
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        let kind = if value.is_empty() { "agent" } else { &value[..end] };
        Self { kind: kind.to_owned() }
    }
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
        Self::from_parts(|key| std::env::var_os(key), CLIENT_NAME.get().map(String::as_str), CLIENT_ACCESS.get().copied())
    }

    /// Pure core of [`current`](Self::current): `var` stands in for the
    /// process env, so tests never read it.
    pub(crate) fn from_parts(
        var: impl Fn(&str) -> Option<OsString>,
        name: Option<&str>,
        access: Option<ControlAccess>,
    ) -> Option<Self> {
        // Presence fails closed: an empty marker still marks an agent. A
        // provider-origin `codemux mcp` (the in-app `codemux` server and
        // per-workspace `.mcp.json` entries) serves an agent too.
        let agent = match var(AGENT_PROCESS_ENV) {
            Some(value) => Some(AgentProcess::from_marker(&value.to_string_lossy())),
            None => var(crate::mcp_server::PROVIDER_ORIGIN_ENV).map(|_| AgentProcess { kind: "mcp".into() }),
        };
        let client = Self { name: name.map(str::to_owned), access, agent };
        (client != Self::default()).then_some(client)
    }

    /// Whether this client asks for less than full access. Such a client must
    /// not talk to a server that would ignore the request (`server_too_old`).
    /// An agent marker sent to an older app keeps that app's behaviour; an
    /// old server cannot be fixed from the client.
    pub(crate) fn narrows(&self) -> bool {
        self.agent.is_none() && matches!(self.access, Some(ControlAccess::ReadOnly | ControlAccess::Supervised))
    }
}

/// Drops an agent identity this process inherited. An app or `codemux serve`
/// started from an agent's shell would otherwise hand it to every terminal it
/// opens. Never call it on CLI paths: they must send the identity.
pub fn scrub_inherited_agent_identity() {
    for key in AGENT_IDENTITY_ENV_VARS {
        if std::env::var_os(key).is_some() {
            // Same pattern and placement as the renderer env scrub: it runs
            // first thing at startup, before other threads read the env.
            unsafe { std::env::remove_var(key) };
        }
    }
}

// ---------------------------------------------------------------------------
// Agent process ancestry
// ---------------------------------------------------------------------------

/// Records a process Codemux just started for an agent (a provider runtime,
/// OpenCode's server, an automation run, the conflict resolver). The socket
/// then treats it and every descendant as an agent process, even when an MCP
/// client cleared the env marker on the way. Linux only; elsewhere the env
/// marker is the only signal.
///
/// Any new code that starts a model with tools outside `JsonRpcChild` must
/// call this and set [`AGENT_PROCESS_ENV`].
pub(crate) fn register_agent_process(pid: u32) {
    #[cfg(target_os = "linux")]
    ancestry::register(pid);
    #[cfg(not(target_os = "linux"))]
    let _ = pid;
}

/// Whether `pid` is a registered agent process or descends from one.
pub(crate) fn is_agent_descendant(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        ancestry::is_descendant(pid)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        false
    }
}

#[cfg(target_os = "linux")]
mod ancestry {
    use std::collections::BTreeMap;
    use std::sync::{Mutex, PoisonError};

    /// A soft bound. Past it, a registration first prunes dead entries; it
    /// never drops or refuses a live one, because that would fail open.
    pub(super) const PRUNE_AT: usize = 4096;
    /// Deeper process trees than this are not walked.
    const MAX_HOPS: usize = 64;

    /// Registered agent processes: pid to start time. The start time keeps a
    /// reused pid from inheriting the mark.
    static ROOTS: Mutex<BTreeMap<u32, u64>> = Mutex::new(BTreeMap::new());

    /// `(parent pid, start time in clock ticks)` from `/proc/<pid>/stat`.
    pub(super) fn stat(pid: u32) -> Option<(u32, u64)> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // The command name (field 2) may contain spaces and parentheses, so
        // fields are counted after its closing ')': state (3), ppid (4), ...,
        // starttime (22).
        let fields: Vec<&str> = stat[stat.rfind(')')? + 1..].split_whitespace().collect();
        Some((fields.get(1)?.parse().ok()?, fields.get(19)?.parse().ok()?))
    }

    pub(super) fn register(pid: u32) {
        // A process that is already gone has no descendants left to mark.
        if let Some((_, start)) = stat(pid) {
            insert(pid, start);
        }
    }

    pub(super) fn insert(pid: u32, start: u64) {
        let mut roots = ROOTS.lock().unwrap_or_else(PoisonError::into_inner);
        if roots.len() >= PRUNE_AT {
            roots.retain(|pid, start| stat(*pid).is_some_and(|(_, live)| live == *start));
        }
        roots.insert(pid, start);
    }

    pub(super) fn is_descendant(pid: u32) -> bool {
        if ROOTS.lock().unwrap_or_else(PoisonError::into_inner).is_empty() {
            return false;
        }
        // Walk up to init or to this app (whose own children are not agents
        // unless registered), reading /proc outside the lock.
        let own = std::process::id();
        let mut chain = Vec::new();
        let mut pid = pid;
        while chain.len() < MAX_HOPS && pid > 1 && pid != own {
            let Some((parent, start)) = stat(pid) else { break };
            chain.push((pid, start));
            pid = parent;
        }
        let roots = ROOTS.lock().unwrap_or_else(PoisonError::into_inner);
        chain.iter().any(|(pid, start)| roots.get(pid) == Some(start))
    }

    #[cfg(test)]
    pub(super) fn len() -> usize {
        ROOTS.lock().unwrap_or_else(PoisonError::into_inner).len()
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
    /// A process started by a Codemux agent (env marker or, on Linux, process
    /// ancestry). Never `CallerKind::Agent`: the socket cannot know the chat's
    /// ceiling, so it rank-compares the process's own `access`.
    Agent { kind: String, access: ControlAccess },
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
            Self::Agent { .. } if required == Access::Admin => Err(ControlError::new(
                ErrorCode::AdminRefused,
                format!(
                    "`{command}` changes who can reach or control Codemux, or what runs without you. \
                     Agent processes cannot run it; ask the person to run it in their own terminal or in Settings."
                ),
            )),
            // Machine-class legacy commands stay open to agents at their own
            // access: full-access chats reach them through codemux-self.
            Self::Agent { access, .. } => narrowed(command, required, *access),
        }
    }
}

/// Resolves who sent a socket request. No client is the owner; a named client
/// is `local:<slug>`, attributed as an outside assistant. An agent marker, or
/// a peer that descends from a registered agent process, wins over any name:
/// a name or access level can then only narrow.
pub(crate) fn resolve_socket_caller(client: Option<ControlClient>, peer_pid: Option<u32>) -> SocketCaller {
    let client = client.unwrap_or_default();
    let access = client.access.unwrap_or(ControlAccess::FullAccess);
    // The kind is only logged; bound what an arbitrary client may send.
    let agent = client
        .agent
        .map(|agent| AgentProcess::from_marker(&agent.kind).kind)
        .or_else(|| peer_pid.filter(|pid| is_agent_descendant(*pid)).map(|_| "process-tree".to_owned()));
    if let Some(kind) = agent {
        return SocketCaller::Agent { kind, access };
    }
    let name = client.name.as_deref().and_then(ClientName::parse);
    SocketCaller::Local(ControlCaller::local(name.as_ref(), access))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use ControlAccess::{FullAccess, ReadOnly, Supervised};

    fn no_env(_: &str) -> Option<OsString> {
        None
    }

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |key| pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| OsString::from(v))
    }

    #[test]
    fn control_client_from_parts_names_and_narrows() {
        assert_eq!(ControlClient::from_parts(no_env, None, None), None, "the owner sends nothing");
        let named = ControlClient::from_parts(no_env, Some("Hermes Agent"), None).unwrap();
        assert_eq!((named.name.as_deref(), named.access, named.agent.as_ref()), (Some("Hermes Agent"), None, None));
        assert!(!named.narrows());
        let narrowed = ControlClient::from_parts(no_env, None, Some(Supervised)).unwrap();
        assert!(narrowed.narrows());
        assert!(ControlClient::from_parts(no_env, None, Some(ReadOnly)).unwrap().narrows());
        assert!(!ControlClient::from_parts(no_env, None, Some(FullAccess)).unwrap().narrows());
    }

    #[test]
    fn control_client_from_parts_marks_agents() {
        let marked = ControlClient::from_parts(env(&[(AGENT_PROCESS_ENV, "")]), None, None).unwrap();
        assert_eq!(marked.agent, Some(AgentProcess { kind: "agent".into() }), "presence alone marks an agent");
        let origin = ControlClient::from_parts(env(&[(crate::mcp_server::PROVIDER_ORIGIN_ENV, "1")]), None, None).unwrap();
        assert_eq!(origin.agent, Some(AgentProcess { kind: "mcp".into() }));
        let both = ControlClient::from_parts(
            env(&[(AGENT_PROCESS_ENV, "opencode"), (crate::mcp_server::PROVIDER_ORIGIN_ENV, "1")]),
            Some("hermes"),
            Some(Supervised),
        )
        .unwrap();
        assert_eq!(both.agent, Some(AgentProcess { kind: "opencode".into() }));
        assert!(!both.narrows(), "an agent never trips the old-server probe");
        let long = ControlClient::from_parts(env(&[(AGENT_PROCESS_ENV, "é123456789012345678901234567890123")]), None, None);
        assert_eq!(long.unwrap().agent.unwrap().kind.len(), 32, "the kind is bounded on a char boundary");
        let gateway = ControlClient::from_parts(env(&[(crate::mcp::gateway::BRIDGE_TOKEN_ENV, "t")]), None, None);
        assert_eq!(gateway, None, "the gateway credential is not a marker; it is scrubbed from terminals");
        assert_eq!(
            serde_json::to_value(&both).unwrap(),
            json!({"name": "hermes", "access": "supervised", "agent": {"kind": "opencode"}})
        );
    }

    #[test]
    fn agent_identity_env_vars_cover_every_marker() {
        assert_eq!(
            AGENT_IDENTITY_ENV_VARS,
            ["CODEMUX_AGENT_PROCESS", "CODEMUX_MCP_GATEWAY_URL", "CODEMUX_MCP_GATEWAY_TOKEN", "CODEMUX_MCP_PROVIDER_ORIGIN"]
        );
    }

    /// The GUI and `codemux serve` drop an inherited identity before anything
    /// else runs. `pty_shell_is_the_owner` stays green while either this scrub
    /// or the PTY strip holds, so each one is pinned on its own.
    #[test]
    fn app_and_serve_scrub_an_inherited_identity_first() {
        for (file, source, signature) in [
            ("lib.rs", include_str!("lib.rs"), "\npub fn run() {"),
            (
                "web_remote/serve.rs",
                include_str!("web_remote/serve.rs"),
                "\npub fn run_serve(opts: ServeOptions) -> Result<(), String> {",
            ),
        ] {
            let start = source.find(signature).unwrap_or_else(|| panic!("{file}: `{}` not found", signature.trim()));
            let first = source[start + signature.len()..]
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty() && !line.starts_with("//"))
                .unwrap();
            assert!(
                first.ends_with("control_identity::scrub_inherited_agent_identity();"),
                "{file}: the first statement must be the scrub, found `{first}`"
            );
        }
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
    fn socket_caller_permission_matrix() {
        let named = ClientName::parse("hermes").unwrap();
        let owner = SocketCaller::owner();
        let supervised = SocketCaller::Local(ControlCaller::local(Some(&named), Supervised));
        let read_only = SocketCaller::Local(ControlCaller::local(Some(&named), ReadOnly));
        let agent = SocketCaller::Agent { kind: "1".into(), access: FullAccess };
        let supervised_agent = SocketCaller::Agent { kind: "1".into(), access: Supervised };
        let read_only_agent = SocketCaller::Agent { kind: "1".into(), access: ReadOnly };
        // (caller, the classes it may run: Read, Write, StartsThreads, Machine, Admin)
        let table: [(&SocketCaller, [bool; 5]); 6] = [
            (&owner, [true, true, true, true, true]),
            (&supervised, [true, true, true, false, false]),
            (&read_only, [true, false, false, false, false]),
            (&agent, [true, true, true, true, false]),
            (&supervised_agent, [true, true, true, false, false]),
            (&read_only_agent, [true, false, false, false, false]),
        ];
        let classes = [Access::Read, Access::Write, Access::StartsThreads, Access::Machine, Access::Admin];
        for (caller, allowed) in table {
            for (class, allowed) in classes.iter().zip(allowed) {
                let result = caller.permits("cmd", *class);
                assert_eq!(result.is_ok(), allowed, "{caller:?} {class:?}");
                if let Err(error) = result {
                    let expected = if matches!(caller, SocketCaller::Agent { .. }) && *class == Access::Admin {
                        ErrorCode::AdminRefused
                    } else {
                        ErrorCode::AccessDenied
                    };
                    assert_eq!(error.code, expected, "{caller:?} {class:?}");
                }
            }
        }
        let refused = read_only.permits("notify", Access::Write).unwrap_err();
        assert_eq!(refused.to_string(), "access_denied: This connection is read-only; `notify` needs write access");
        let refused = supervised.permits("create_workspace", Access::Machine).unwrap_err();
        assert_eq!(refused.to_string(), "access_denied: This connection is supervised; `create_workspace` needs full access");
        let refused = agent.permits("agent_connector_approve", Access::Admin).unwrap_err();
        assert!(refused.to_string().starts_with("admin_refused: `agent_connector_approve` changes who can reach"), "{refused}");
    }

    fn local_caller(caller: SocketCaller) -> ControlCaller {
        match caller {
            SocketCaller::Local(caller) => caller,
            other => panic!("expected a local caller, got {other:?}"),
        }
    }

    fn client(name: Option<&str>, access: Option<ControlAccess>, agent: Option<&str>) -> Option<ControlClient> {
        Some(ControlClient { name: name.map(Into::into), access, agent: agent.map(|kind| AgentProcess { kind: kind.into() }) })
    }

    #[test]
    fn socket_clients_resolve_to_local_principals() {
        let owner = local_caller(resolve_socket_caller(None, None));
        assert_eq!(owner.principal, "local");
        assert_eq!(owner.access, FullAccess);
        let narrowed = local_caller(resolve_socket_caller(client(None, Some(ReadOnly), None), None));
        assert_eq!((narrowed.principal.as_str(), narrowed.access), ("local", ReadOnly));
        let named = local_caller(resolve_socket_caller(client(Some("Hermes Agent"), Some(Supervised), None), None));
        assert_eq!(named.principal, "local:hermes-agent");
        assert_eq!(named.label.as_deref(), Some("Hermes Agent"));
        assert_eq!(named.access, Supervised);
        let junk = local_caller(resolve_socket_caller(client(Some("---"), None, None), None));
        assert_eq!(junk.principal, "local", "an unusable name is the owner");
        let own_pid = local_caller(resolve_socket_caller(None, Some(std::process::id())));
        assert_eq!(own_pid.principal, "local", "the app itself is not an agent");
    }

    #[test]
    fn agent_markers_win_over_names() {
        let marked = resolve_socket_caller(client(Some("hermes"), Some(Supervised), Some("opencode")), None);
        assert!(matches!(&marked, SocketCaller::Agent { kind, access: Supervised } if kind == "opencode"), "{marked:?}");
        let default_access = resolve_socket_caller(client(None, None, Some("1")), None);
        assert!(matches!(default_access, SocketCaller::Agent { access: FullAccess, .. }));
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

    #[cfg(target_os = "linux")]
    mod ancestry_tests {
        use super::super::*;
        use std::io::BufRead;
        use std::process::{Child, Command, Stdio};

        /// Stops a test-started process by its pid.
        struct Started(Child);
        impl Drop for Started {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        fn sleeper() -> Started {
            Started(Command::new("sleep").arg("30").spawn().unwrap())
        }

        #[test]
        fn agent_descendant_is_detected_through_proc_ancestry() {
            let mut shell = Started(
                Command::new("sh")
                    .args(["-c", "sleep 30 & echo $!; wait"])
                    .stdout(Stdio::piped())
                    .spawn()
                    .unwrap(),
            );
            let mut line = String::new();
            std::io::BufReader::new(shell.0.stdout.take().unwrap()).read_line(&mut line).unwrap();
            let sleep_pid: u32 = line.trim().parse().unwrap();
            assert!(!is_agent_descendant(sleep_pid), "nothing is registered for this tree yet");
            register_agent_process(shell.0.id());
            assert!(is_agent_descendant(shell.0.id()), "the registered process itself");
            assert!(is_agent_descendant(sleep_pid), "its child");
            assert!(!is_agent_descendant(std::process::id()), "the app is never its own agent");
            assert!(!is_agent_descendant(1));
            // The grandchild was started by this test; stop it by pid.
            unsafe { libc::kill(sleep_pid as i32, libc::SIGTERM) };
        }

        #[test]
        fn peer_pid_reuse_is_not_an_agent() {
            let unregistered = sleeper();
            assert!(!is_agent_descendant(unregistered.0.id()));
            let reused = sleeper();
            let (_, start) = ancestry::stat(reused.0.id()).unwrap();
            // An earlier process with this pid, which started at another time.
            ancestry::insert(reused.0.id(), start + 1);
            assert!(!is_agent_descendant(reused.0.id()), "a reused pid does not inherit the mark");
            register_agent_process(reused.0.id());
            assert!(is_agent_descendant(reused.0.id()), "registering the live process replaces the stale entry");
        }

        #[test]
        fn registrations_past_the_bound_keep_live_processes() {
            let live = sleeper();
            register_agent_process(live.0.id());
            // Dead entries: pids above the kernel's pid_max never exist.
            for pid in 0..ancestry::PRUNE_AT as u32 {
                ancestry::insert(u32::MAX - pid, 1);
            }
            let fresh = sleeper();
            register_agent_process(fresh.0.id());
            assert!(ancestry::len() < ancestry::PRUNE_AT, "the dead entries were pruned");
            assert!(is_agent_descendant(live.0.id()), "a live registration is never dropped");
            assert!(is_agent_descendant(fresh.0.id()));
        }

        #[test]
        fn proc_stat_parses_names_with_parentheses() {
            let (parent, start) = ancestry::stat(std::process::id()).unwrap();
            assert!(parent > 0 && start > 0);
            // The kernel names a process after the path it was started from.
            let dir = tempfile::tempdir().unwrap();
            let odd = dir.path().join("odd ) name (");
            std::os::unix::fs::symlink(which::which("sleep").unwrap(), &odd).unwrap();
            let tricky = Started(Command::new(&odd).arg("30").spawn().unwrap());
            let comm = format!("/proc/{}/comm", tricky.0.id());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while std::fs::read_to_string(&comm).unwrap_or_default().trim_end() != "odd ) name (" {
                assert!(std::time::Instant::now() < deadline, "the process never took its odd name");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let (parent, _) = ancestry::stat(tricky.0.id()).unwrap();
            assert_eq!(parent, std::process::id());
        }
    }
}
