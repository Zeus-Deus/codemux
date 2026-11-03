//! `codemux capabilities`: every command, flag, control tool, exit code and
//! environment variable as JSON, generated from the command tree and the
//! tool registry, so it cannot drift from them. Needs no running app.

use super::Cli;
use clap::{Arg, ArgAction, Command, CommandFactory};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Serialize)]
pub(crate) struct Capabilities {
    /// First: `install.sh` reads the first `"version"` line.
    pub(crate) version: &'static str,
    pub(crate) global_args: Vec<ArgInfo>,
    pub(crate) commands: BTreeMap<String, CommandInfo>,
    pub(crate) tools: Vec<ToolSummary>,
    pub(crate) exit_codes: Vec<ExitCodeInfo>,
    pub(crate) environment: BTreeMap<&'static str, &'static str>,
}

#[derive(Serialize)]
pub(crate) struct CommandInfo {
    pub(crate) description: Option<String>,
    pub(crate) usage: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) aliases: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) args: Vec<ArgInfo>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) subcommands: BTreeMap<String, CommandInfo>,
}

#[derive(Serialize)]
pub(crate) struct ArgInfo {
    pub(crate) name: String,
    pub(crate) long: Option<String>,
    pub(crate) short: Option<char>,
    pub(crate) positional: bool,
    pub(crate) required: bool,
    pub(crate) flag: bool,
    pub(crate) multiple: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) value_names: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) possible_values: Vec<String>,
    pub(crate) default: Option<String>,
    pub(crate) env: Option<String>,
    pub(crate) help: Option<String>,
}

/// A control tool this build ships, as an outside caller at full access
/// sees it. `codemux tools` lists what a running app lets this caller run.
#[derive(Serialize)]
pub(crate) struct ToolSummary {
    pub(crate) name: &'static str,
    /// The least `--access` that runs it.
    pub(crate) access: &'static str,
    pub(crate) read_only: bool,
    pub(crate) description: &'static str,
}

#[derive(Serialize)]
pub(crate) struct ExitCodeInfo {
    pub(crate) code: i32,
    pub(crate) meaning: &'static str,
}

/// Variables the CLI reads or Codemux sets. Not derivable from the tree.
const ENVIRONMENT: &[(&str, &str)] = &[
    ("CODEMUX", "Set to '1' when running inside Codemux"),
    ("CODEMUX_VERSION", "Codemux version"),
    ("CODEMUX_WORKSPACE_ID", "Current workspace ID; the default --workspace of the thread verbs"),
    ("CODEMUX_PANE_ID", "Current pane ID (the default target for `codemux monitor`)"),
    ("CODEMUX_BROWSER_CMD", "Command prefix for browser control"),
    ("CODEMUX_PASSWORD", "Password for a non-interactive `codemux login` (warns; for automated runs only)"),
    ("CODEMUX_API_URL", "Override the Codemux API base URL (default https://api.codemux.org)"),
    ("CODEMUX_CLIENT", "Default --client: the name the control socket records your calls under"),
    ("CODEMUX_ACCESS", "Default --access: read-only, supervised or full-access"),
    ("CODEMUX_AGENT_PROCESS", "Set by Codemux on processes it starts for agents; the control socket refuses them admin commands"),
    ("CODEMUX_MCP_GATEWAY_URL", "Set by Codemux in agent processes, with CODEMUX_MCP_GATEWAY_TOKEN; `codemux mcp-call` uses them to call the chat's own tools"),
    ("BROWSER", "Set to 'codemux browser open' for URL handling"),
];

const EXIT_CODES: &[(i32, &str)] = &[
    (0, "done: the command succeeded, and a waited run completed"),
    (1, "failed: an error, a refusal, or a waited run that failed"),
    (2, "needs input: a waited run is blocked on an approval, a question or a secret card"),
    (3, "timed out: --timeout elapsed while the work kept going"),
    (64, "usage: the arguments were rejected"),
];

pub(crate) fn capabilities() -> Capabilities {
    let mut root = Cli::command();
    root.build();
    let tools = crate::agent_control::registry::outside_tool_index()
        .into_iter()
        .map(|(name, access, description)| ToolSummary {
            name,
            access: super::args::access_label(access),
            read_only: access == crate::agent_control::ControlAccess::ReadOnly,
            description,
        })
        .collect();
    Capabilities {
        version: env!("CARGO_PKG_VERSION"),
        global_args: arguments(&root),
        commands: subcommands(&root),
        tools,
        exit_codes: EXIT_CODES.iter().map(|&(code, meaning)| ExitCodeInfo { code, meaning }).collect(),
        environment: ENVIRONMENT.iter().copied().collect(),
    }
}

fn subcommands(parent: &Command) -> BTreeMap<String, CommandInfo> {
    parent
        .get_subcommands()
        .filter(|command| !command.is_hide_set() && command.get_name() != "help")
        .map(|command| {
            let usage = command.clone().render_usage().to_string();
            let info = CommandInfo {
                description: command.get_about().map(|about| about.to_string()),
                usage: usage.strip_prefix("Usage: ").unwrap_or(&usage).to_owned(),
                aliases: command.get_visible_aliases().map(str::to_owned).collect(),
                args: arguments(command),
                subcommands: subcommands(command),
            };
            (command.get_name().to_owned(), info)
        })
        .collect()
}

/// A command's own arguments. Help, version and propagated globals are left
/// out; globals are listed once, on the root.
fn arguments(command: &Command) -> Vec<ArgInfo> {
    command
        .get_arguments()
        .filter(|arg| !arg.is_hide_set() && !matches!(arg.get_action(), ArgAction::Help | ArgAction::HelpShort | ArgAction::HelpLong | ArgAction::Version))
        .filter(|arg| command.get_name() == "codemux" || !arg.is_global_set())
        .map(arg_info)
        .collect()
}

fn arg_info(arg: &Arg) -> ArgInfo {
    let flag = matches!(arg.get_action(), ArgAction::SetTrue | ArgAction::SetFalse | ArgAction::Count);
    let multiple = matches!(arg.get_action(), ArgAction::Append) || arg.get_num_args().is_some_and(|range| range.max_values() > 1);
    ArgInfo {
        name: arg.get_id().to_string(),
        long: arg.get_long().map(str::to_owned),
        short: arg.get_short(),
        positional: arg.is_positional(),
        required: arg.is_required_set(),
        flag,
        multiple,
        value_names: arg.get_value_names().into_iter().flatten().map(|name| name.to_string()).collect(),
        possible_values: if flag {
            Vec::new()
        } else {
            arg.get_possible_values().iter().filter(|value| !value.is_hide_set()).map(|value| value.get_name().to_owned()).collect()
        },
        default: (!flag).then(|| arg.get_default_values().iter().map(|value| value.to_string_lossy().into_owned()).collect::<Vec<_>>().join(","))
            .filter(|default| !default.is_empty()),
        env: arg.get_env().map(|env| env.to_string_lossy().into_owned()),
        help: arg.get_help().map(|help| help.to_string()),
    }
}

/// Prints [`capabilities`] as pretty JSON.
pub(crate) fn print() -> Result<(), String> {
    super::output::emit_json(&capabilities())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every visible command path, `thread start` style.
    fn visible_paths(command: &Command, prefix: &str, paths: &mut Vec<String>) {
        for sub in command.get_subcommands().filter(|sub| !sub.is_hide_set() && sub.get_name() != "help") {
            let path = format!("{prefix}{}", sub.get_name());
            paths.push(path.clone());
            visible_paths(sub, &format!("{path} "), paths);
        }
    }

    fn lookup<'a>(commands: &'a BTreeMap<String, CommandInfo>, path: &str) -> Option<&'a CommandInfo> {
        let mut parts = path.split(' ');
        let mut found = commands.get(parts.next()?)?;
        for part in parts {
            found = found.subcommands.get(part)?;
        }
        Some(found)
    }

    #[test]
    fn capabilities_list_every_visible_command() {
        let caps = capabilities();
        let mut root = Cli::command();
        root.build();
        let mut paths = Vec::new();
        visible_paths(&root, "", &mut paths);
        assert!(paths.len() > 40, "{paths:?}");
        for path in &paths {
            assert!(lookup(&caps.commands, path).is_some(), "{path} is missing from capabilities");
        }
        for internal in ["mcp-bridge", "pty-daemon"] {
            assert!(!caps.commands.contains_key(internal), "{internal} is internal");
        }
        for verb in ["tools", "call", "thread start", "thread wait", "workspace create", "worktree create", "connector issue"] {
            assert!(lookup(&caps.commands, verb).is_some(), "{verb}");
        }
        let start = lookup(&caps.commands, "thread start").unwrap();
        assert!(start.usage.starts_with("codemux thread start "), "{}", start.usage);
        let mode = start.args.iter().find(|arg| arg.long.as_deref() == Some("permission-mode")).unwrap();
        assert_eq!(mode.value_names, ["MODE"]);
        // The caller flags sit on the root once.
        let client = caps.global_args.iter().find(|arg| arg.long.as_deref() == Some("client")).unwrap();
        assert_eq!(client.env.as_deref(), Some("CODEMUX_CLIENT"));
        let access = caps.global_args.iter().find(|arg| arg.long.as_deref() == Some("access")).unwrap();
        assert_eq!(access.possible_values, ["read-only", "supervised", "full-access"]);
    }

    #[test]
    fn capabilities_version_is_first() {
        let printed = serde_json::to_string_pretty(&capabilities()).unwrap();
        assert_eq!(printed.lines().nth(1), Some(format!("  \"version\": \"{}\",", env!("CARGO_PKG_VERSION")).as_str()));
    }

    #[test]
    fn capabilities_include_registry_tools_and_exit_codes() {
        let caps = capabilities();
        let names: Vec<&str> = caps.tools.iter().map(|tool| tool.name).collect();
        for name in ["thread_launch", "thread_wait", "workspace_list"] {
            assert!(names.contains(&name), "{name} in {names:?}");
        }
        let wait = caps.tools.iter().find(|tool| tool.name == "thread_wait").unwrap();
        assert_eq!((wait.access, wait.read_only), ("read-only", true));
        let launch = caps.tools.iter().find(|tool| tool.name == "thread_launch").unwrap();
        assert_eq!((launch.access, launch.read_only), ("supervised", false));
        let codes: Vec<i32> = caps.exit_codes.iter().map(|code| code.code).collect();
        assert_eq!(codes, [0, 1, 2, 3, 64]);
        for name in ["CODEMUX_CLIENT", "CODEMUX_ACCESS", "CODEMUX_WORKSPACE_ID"] {
            assert!(caps.environment.contains_key(name), "{name}");
        }
    }

    #[test]
    fn every_visible_command_has_a_description() {
        let mut root = Cli::command();
        root.build();
        let mut paths = Vec::new();
        visible_paths(&root, "", &mut paths);
        let caps = capabilities();
        for path in paths {
            let info = lookup(&caps.commands, &path).unwrap();
            assert!(info.description.as_deref().is_some_and(|about| !about.trim().is_empty()), "`codemux {path}` has no description");
        }
    }
}
