//! Codemux's tools for Claude Code typed in a Codemux terminal.
//!
//! Codemux never writes MCP config where an unrelated agent would read it.
//! Instead it keeps a tiny Claude Code plugin in its own data directory and
//! points terminals it starts at it with `CLAUDE_CODE_PLUGIN_DIRS`. Only a
//! `claude` started in such a terminal (and the sessions it starts itself)
//! loads it; one started anywhere else on the machine never sees it.
//!
//! - The plugin's server runs `codemux mcp` with `CODEMUX_AGENT_PROCESS=terminal`,
//!   so it serves the agent tools and the workspace comes from the shell's
//!   own `CODEMUX_WORKSPACE_ID`. No workspace is baked into the shared file.
//! - Claude Code names its tools `mcp__plugin_codemux_codemux__<tool>`.
//! - `--strict-mcp-config` drops plugin servers too, and
//!   `CLAUDE_CODE_PLUGIN_DIRS= claude` runs a Claude without them.
//! - A remote host's PTY daemon keeps its own copy that runs the host's
//!   `codemux-remote mcp`; no desktop path ever reaches a host.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The env var Claude Code reads extra plugin directories from.
pub const PLUGIN_DIRS_ENV: &str = "CLAUDE_CODE_PLUGIN_DIRS";

/// The agent marker the plugin's local `codemux mcp` runs under.
pub const TERMINAL_AGENT_KIND: &str = "terminal";

/// Directory name of the plugin under each Codemux data directory.
const PLUGIN_DIR_NAME: &str = "claude-plugin";

/// The app data directories (release, dev) whose plugin is Codemux's.
const APP_DATA_DIRS: [&str; 2] = ["codemux", "codemux-dev"];

#[cfg(windows)]
const SEPARATOR: char = ';';
#[cfg(not(windows))]
const SEPARATOR: char = ':';

/// Write a plugin into `dir` whose `codemux` MCP server runs `command args`
/// with `env`. Each file is replaced atomically and only when it changed.
pub fn ensure_plugin(dir: &Path, command: &str, args: &[String], env: &[(&str, &str)]) -> std::io::Result<()> {
    let manifest = serde_json::json!({
        "name": "codemux",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Codemux workspace tools for Claude Code started in a Codemux terminal",
    });
    let mut server = serde_json::json!({ "command": command, "args": args });
    if !env.is_empty() {
        server["env"] = env.iter().map(|(key, value)| ((*key).to_owned(), serde_json::json!(value))).collect();
    }
    let servers = serde_json::json!({ "mcpServers": { "codemux": server } });
    crate::config_file::write_if_changed(&dir.join(".claude-plugin").join("plugin.json"), &pretty(&manifest))?;
    crate::config_file::write_if_changed(&dir.join(".mcp.json"), &pretty(&servers))?;
    Ok(())
}

fn pretty(value: &serde_json::Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(value).expect("JSON values serialize");
    bytes.push(b'\n');
    bytes
}

/// This app's plugin, written on first use and refreshed once per process so
/// it always names the running binary. `None` when it could not be written;
/// terminals then start without it.
pub fn local_plugin_dir() -> Option<&'static Path> {
    static PREPARED: OnceLock<Option<PathBuf>> = OnceLock::new();
    PREPARED.get_or_init(prepare_local_plugin).as_deref()
}

fn prepare_local_plugin() -> Option<PathBuf> {
    // Tests write their plugin only inside an isolated root, never into the
    // developer's own data directory.
    #[cfg(test)]
    crate::test_isolation::isolated_root()?;
    let dir = plugin_dir_in(&dirs::data_dir()?.join(crate::APP_DIR_NAME));
    let command = crate::mcp_server::codemux_mcp_command();
    let env = [(crate::control_identity::AGENT_PROCESS_ENV, TERMINAL_AGENT_KIND)];
    match ensure_plugin(&dir, &command, &["mcp".to_owned()], &env) {
        Ok(()) => Some(dir),
        Err(error) => {
            log::warn!("[codemux::claude_plugin] terminals start without Codemux tools for Claude: {error}");
            None
        }
    }
}

/// Where Codemux keeps its plugin under one of its data directories. Both
/// the writers and [`owned_plugin_dirs`] derive paths from it.
fn plugin_dir_in(data_dir: &Path) -> PathBuf {
    data_dir.join(PLUGIN_DIR_NAME)
}

/// Every plugin directory this user's Codemux writes: the release and dev
/// app's, and the remote host's under its default state root. Only these
/// exact paths are Codemux's; a user directory that merely ends in
/// `codemux/claude-plugin` is not.
fn owned_plugin_dirs() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = dirs::data_dir()
        .map(|data| APP_DATA_DIRS.iter().map(|name| data.join(name)).collect())
        .unwrap_or_default();
    #[cfg(unix)]
    roots.push(crate::remote::config::default_state_dir());
    roots.iter().map(|root| plugin_dir_in(root)).collect()
}

/// The plugin a remote host's terminals get: the host's own
/// `codemux-remote mcp` for the daemon state in the default state root,
/// the one place [`owned_plugin_dirs`] recognizes as the host's.
#[cfg(unix)]
pub fn ensure_remote_host_plugin(codemux_remote: &Path) -> std::io::Result<PathBuf> {
    ensure_remote_host_plugin_in(&crate::remote::config::default_state_dir(), codemux_remote)
}

#[cfg(unix)]
fn ensure_remote_host_plugin_in(state_dir: &Path, codemux_remote: &Path) -> std::io::Result<PathBuf> {
    let dir = plugin_dir_in(state_dir);
    let args = ["mcp".to_owned(), "--state-dir".to_owned(), state_dir.display().to_string()];
    ensure_plugin(&dir, &codemux_remote.display().to_string(), &args, &[])?;
    Ok(dir)
}

/// The `CLAUDE_CODE_PLUGIN_DIRS` value for a terminal: `ours` first, then the
/// inherited directories that are not Codemux's own.
pub fn plugin_dirs_value(ours: &Path, inherited: Option<&OsStr>) -> OsString {
    plugin_dirs_value_with(ours, inherited, &owned_plugin_dirs())
}

fn plugin_dirs_value_with(ours: &Path, inherited: Option<&OsStr>, owned: &[PathBuf]) -> OsString {
    let mut value = OsString::from(ours);
    for dir in inherited.map(split).unwrap_or_default() {
        if !is_owned(&dir, owned) && !same_dir(&dir, ours) {
            value.push(SEPARATOR.to_string());
            value.push(dir);
        }
    }
    value
}

/// Removes every Codemux plugin directory from this process's inherited
/// `CLAUDE_CODE_PLUGIN_DIRS`. An app or `codemux serve` started from a
/// Codemux terminal would otherwise hand the terminal tools to its own chat
/// sidecars and helper agents. Call it first thing at startup, next to
/// [`crate::control_identity::scrub_inherited_agent_identity`].
pub fn scrub_inherited_plugin_dirs() {
    let Some(kept) = inherited_without_codemux_dirs() else {
        return;
    };
    // Same placement as the identity scrub: before other threads read the env.
    unsafe {
        if kept.is_empty() {
            std::env::remove_var(PLUGIN_DIRS_ENV);
        } else {
            std::env::set_var(PLUGIN_DIRS_ENV, kept);
        }
    }
}

/// This process's `CLAUDE_CODE_PLUGIN_DIRS` without Codemux's directories, or
/// `None` when it holds none. Every agent child Codemux spawns gets this
/// value (see `json_rpc_child`), so a runtime started from a Codemux
/// terminal never hands that terminal's tools to the agents it launches;
/// they get their own tools per session instead.
pub fn inherited_without_codemux_dirs() -> Option<OsString> {
    without_codemux_dirs(&std::env::var_os(PLUGIN_DIRS_ENV)?, &owned_plugin_dirs())
}

/// `value` without the `owned` directories, or `None` when it had none.
fn without_codemux_dirs(value: &OsStr, owned: &[PathBuf]) -> Option<OsString> {
    let dirs = split(value);
    if !dirs.iter().any(|dir| is_owned(dir, owned)) {
        return None;
    }
    let mut kept = OsString::new();
    for dir in dirs.into_iter().filter(|dir| !is_owned(dir, owned)) {
        if !kept.is_empty() {
            kept.push(SEPARATOR.to_string());
        }
        kept.push(dir);
    }
    Some(kept)
}

fn split(value: &OsStr) -> Vec<PathBuf> {
    value
        .to_string_lossy()
        .split(SEPARATOR)
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn is_owned(dir: &Path, owned: &[PathBuf]) -> bool {
    owned.iter().any(|ours| same_dir(dir, ours))
}

/// The same path as Codemux spells it: a trailing separator or `.` does not
/// matter, and on Windows neither do case and a `\\?\` prefix (as
/// `dunce` strips it elsewhere). No filesystem access: this runs on every
/// agent spawn, and Codemux writes the value from the same data paths.
fn same_dir(a: &Path, b: &Path) -> bool {
    let (a, b) = (dunce::simplified(a), dunce::simplified(b));
    #[cfg(windows)]
    {
        let key = |path: &Path| path.components().map(|part| part.as_os_str().to_string_lossy().to_lowercase()).collect::<Vec<_>>();
        key(a) == key(b)
    }
    #[cfg(not(windows))]
    {
        a == b
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn read(path: &Path) -> Value {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn plugin_runs_the_terminal_agent_server_without_a_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = dir.path().join("codemux-dev").join(PLUGIN_DIR_NAME);
        ensure_plugin(&plugin, "/opt/codemux", &["mcp".into()], &[("CODEMUX_AGENT_PROCESS", "terminal")]).unwrap();
        assert_eq!(read(&plugin.join(".claude-plugin/plugin.json"))["name"], "codemux");
        assert_eq!(
            read(&plugin.join(".mcp.json")),
            json!({"mcpServers": {"codemux": {
                "command": "/opt/codemux", "args": ["mcp"], "env": {"CODEMUX_AGENT_PROCESS": "terminal"}
            }}})
        );
        assert!(!std::fs::read_to_string(plugin.join(".mcp.json")).unwrap().contains("CODEMUX_WORKSPACE_ID"));
        assert!(!std::fs::read_to_string(plugin.join(".mcp.json")).unwrap().contains("CODEMUX_MCP_PROVIDER_ORIGIN"));
    }

    #[test]
    fn plugin_is_rewritten_only_when_it_changes() {
        let dir = tempfile::tempdir().unwrap();
        let plugin = dir.path().join(PLUGIN_DIR_NAME);
        ensure_plugin(&plugin, "/a/codemux", &["mcp".into()], &[]).unwrap();
        let before = std::fs::metadata(plugin.join(".mcp.json")).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        ensure_plugin(&plugin, "/a/codemux", &["mcp".into()], &[]).unwrap();
        assert_eq!(std::fs::metadata(plugin.join(".mcp.json")).unwrap().modified().unwrap(), before);
        ensure_plugin(&plugin, "/b/codemux", &["mcp".into()], &[]).unwrap();
        assert_eq!(read(&plugin.join(".mcp.json"))["mcpServers"]["codemux"]["command"], "/b/codemux");
    }

    #[cfg(unix)]
    #[test]
    fn remote_host_plugin_runs_the_hosts_codemux_remote() {
        let state = tempfile::tempdir().unwrap();
        let dir = ensure_remote_host_plugin_in(state.path(), Path::new("/home/u/.local/bin/codemux-remote")).unwrap();
        assert_eq!(dir, state.path().join(PLUGIN_DIR_NAME));
        assert_eq!(
            read(&dir.join(".mcp.json"))["mcpServers"]["codemux"],
            json!({"command": "/home/u/.local/bin/codemux-remote", "args": ["mcp", "--state-dir", state.path().display().to_string()]})
        );
    }

    /// What Codemux writes for a user whose data dir is `/h/.local/share`.
    #[cfg(unix)]
    fn owned() -> Vec<PathBuf> {
        ["codemux", "codemux-dev", "codemux-remote"].iter().map(|name| plugin_dir_in(&Path::new("/h/.local/share").join(name))).collect()
    }

    #[cfg(unix)]
    #[test]
    fn terminal_value_puts_ours_first_and_drops_other_codemux_dirs() {
        let ours = Path::new("/h/.local/share/codemux/claude-plugin");
        assert_eq!(plugin_dirs_value_with(ours, None, &owned()), OsString::from("/h/.local/share/codemux/claude-plugin"));
        assert_eq!(plugin_dirs_value_with(ours, Some(OsStr::new("")), &owned()), OsString::from(ours));
        let inherited = OsStr::new("/u/mine:/h/.local/share/codemux-dev/claude-plugin/:/h/.local/share/codemux/claude-plugin::/u/other");
        assert_eq!(
            plugin_dirs_value_with(ours, Some(inherited), &owned()),
            OsString::from("/h/.local/share/codemux/claude-plugin:/u/mine:/u/other")
        );
    }

    /// Only the exact paths Codemux writes are its own: a user's checkout
    /// whose path merely ends the same way stays.
    #[cfg(unix)]
    #[test]
    fn startup_scrub_keeps_only_the_users_dirs() {
        assert_eq!(without_codemux_dirs(OsStr::new("/u/mine:/u/claude-plugin"), &owned()), None, "nothing of ours");
        assert_eq!(
            without_codemux_dirs(OsStr::new("/work/plugins/codemux/claude-plugin:/u/codemux-remote/claude-plugin"), &owned()),
            None,
            "same names elsewhere are the user's"
        );
        assert_eq!(
            without_codemux_dirs(OsStr::new("/h/.local/share/codemux/claude-plugin:/work/plugins/codemux/claude-plugin:/u/mine"), &owned()),
            Some(OsString::from("/work/plugins/codemux/claude-plugin:/u/mine"))
        );
        assert_eq!(
            without_codemux_dirs(OsStr::new("/h/.local/share/codemux-remote/claude-plugin"), &owned()),
            Some(OsString::new()),
            "an empty result removes the variable"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_match_regardless_of_case_and_prefix() {
        let ours = Path::new(r"C:\Users\u\AppData\Roaming\codemux\claude-plugin");
        for spelling in [r"c:\users\U\appdata\roaming\Codemux\claude-plugin", r"\\?\C:\Users\u\AppData\Roaming\codemux\claude-plugin", "C:/Users/u/AppData/Roaming/codemux/claude-plugin"] {
            assert!(same_dir(Path::new(spelling), ours), "{spelling}");
        }
        assert!(!same_dir(Path::new(r"D:\work\codemux\claude-plugin"), ours));
    }
}
