//! Removes the MCP registrations older Codemux builds wrote outside
//! Codemux's own data.
//!
//! Those builds wrote a `codemux` server into `<workspace>/.mcp.json` (the
//! home folder included), and `codemux-remote serve` wrote one into
//! `~/.claude.json`. Claude Code reads `.mcp.json` from parent folders too, so
//! every agent started below such a folder got Codemux's tools. Codemux now
//! delivers its tools per launch (see `claude_plugin`), so those entries are
//! only stale leftovers.
//!
//! Only an entry with exactly the shape Codemux wrote, carrying its marker env
//! and naming a Codemux executable, is removed. Anything a user could have
//! written by hand (a bare `codemux mcp`, `--client`, other keys) is kept and,
//! when it looks like Codemux, logged as ambiguous. The `~/.claude.json`
//! entry is withdrawn only where the Codemux app has run, as 0.27.0 did.
//! `~/.vexis` is never read or touched. Symlinks, unreadable and invalid
//! files are skipped. Every run is idempotent and cheap, so the full run
//! repeats on each start, and [`before_launch`] covers a folder opened later
//! before its first terminal or chat starts.

use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const SERVER: &str = "codemux";

/// Codemux's worktree roots under HOME, scanned two levels deep
/// (`<root>/<repo>/<branch>`).
const WORKTREE_ROOTS: [&str; 2] = [".codemux/worktrees", ".codemux-dev/worktrees"];

/// Bounds a worktree-root scan, so a huge or slow tree cannot stall it.
const MAX_SCANNED_DIRS: usize = 20_000;

/// What the cleanup did to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// No `codemux` entry, or a file that was skipped silently (missing).
    Untouched,
    /// Our entry was removed; the file was rewritten.
    Removed,
    /// Our entry was the file's only content; the file was deleted.
    Deleted,
    /// A `codemux` entry that runs Codemux but carries no proof Codemux
    /// wrote it. Kept; the reason says why.
    Ambiguous(String),
    /// The file could not be cleaned safely (symlink, unreadable, invalid,
    /// changed meanwhile). Kept as it is.
    Skipped(String),
}

/// Whether a project `.mcp.json` `codemux` entry is one an older Codemux
/// wrote: exactly `{command, args: ["mcp"], env: {<markers>}}` (plus an
/// optional `type: "stdio"`), with a valid marker value and a Codemux
/// executable. The markers are `CODEMUX_WORKSPACE_ID` (the first builds,
/// 2026-03) and `CODEMUX_MCP_PROVIDER_ORIGIN=1` (the last). A marker alone
/// never identifies an arbitrary program.
pub fn is_generated_project_entry(entry: &Value) -> bool {
    let Some(entry) = entry.as_object() else {
        return false;
    };
    let Some(Value::Object(env)) = entry.get("env") else {
        return false;
    };
    only_keys(entry, &["command", "args", "env", "type"])
        && stdio_type(entry)
        && mcp_args(entry)
        && !env.is_empty()
        && env.iter().all(|(key, value)| match key.as_str() {
            crate::control_identity::LEGACY_PROVIDER_ORIGIN_ENV => value == "1",
            "CODEMUX_WORKSPACE_ID" => value.as_str().is_some_and(|id| !id.trim().is_empty()),
            _ => false,
        })
        && entry.get("command").and_then(Value::as_str).is_some_and(is_codemux_command)
}

/// Whether a `codemux` entry looks like Codemux but is not provably
/// generated: kept, and reported so the user can decide.
fn looks_like_codemux(entry: &Value) -> bool {
    entry.get("command").and_then(Value::as_str).is_some_and(is_codemux_command)
}

/// Whether a `~/.claude.json` top-level `codemux` entry is exactly the one
/// `codemux-remote serve` registered: `{command: <…/codemux-remote>, args:
/// ["mcp"]}` and nothing else. `claude mcp add` writes the same server with
/// `type` and `env` added, so an entry with those keys is the user's own.
pub fn is_registered_remote_entry(entry: &Value) -> bool {
    let Some(entry) = entry.as_object() else {
        return false;
    };
    entry.len() == 2
        && mcp_args(entry)
        && entry
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(|command| basename(command) == "codemux-remote")
}

/// Whether the Codemux app (desktop or `codemux serve`) has run here as this
/// user: its window layout in `<config>/codemux*/layout.json`. Only then is
/// the global `codemux` entry the app's, so only then is `codemux-remote`'s
/// old registration withdrawn; on a host that runs only `codemux-remote`,
/// such an entry is how the user reaches it and stays. codemux-remote never
/// writes that file.
pub fn desktop_has_run(config_dir: Option<&Path>) -> bool {
    // Both names whichever build this is: a release helper must see a dev
    // desktop's layout, and a dev helper a release desktop's.
    config_dir.is_some_and(|dir| {
        ["codemux", "codemux-dev"].iter().any(|name| dir.join(name).join("layout.json").is_file())
    })
}

fn only_keys(entry: &Map<String, Value>, allowed: &[&str]) -> bool {
    entry.keys().all(|key| allowed.contains(&key.as_str()))
}

fn stdio_type(entry: &Map<String, Value>) -> bool {
    entry.get("type").is_none_or(|kind| kind == "stdio")
}

fn mcp_args(entry: &Map<String, Value>) -> bool {
    entry.get("args").is_some_and(|args| *args == serde_json::json!(["mcp"]))
}

/// The last path component, for `/` and `\` paths alike.
fn basename(command: &str) -> &str {
    command.rsplit(['/', '\\']).next().unwrap_or(command)
}

/// A Codemux executable by name: `codemux`, `codemux.exe`, `codemux-remote`,
/// a Codemux AppImage, or a `codemux_lib-<hash>` test binary.
fn is_codemux_command(command: &str) -> bool {
    let name = basename(command);
    let lower = name.to_ascii_lowercase();
    matches!(lower.as_str(), "codemux" | "codemux.exe" | "codemux-remote")
        || (lower.ends_with(".appimage") && lower.contains("codemux"))
        || name.starts_with("codemux_lib-")
}

/// Cleans `<dir>/.mcp.json`: removes a generated `codemux` entry, drops an
/// emptied `mcpServers`, and deletes the file only when nothing else is left.
pub fn clean_project_file(dir: &Path) -> Outcome {
    let path = dir.join(".mcp.json");
    edit(&path, true, |servers| match servers.get(SERVER) {
        Some(entry) if is_generated_project_entry(entry) => Ok(true),
        Some(entry) if looks_like_codemux(entry) => Err(Outcome::Ambiguous(
            "a `codemux` entry without the marker Codemux wrote; it may be yours".into(),
        )),
        _ => Ok(false),
    })
}

/// Withdraws `codemux-remote serve`'s registration from `<home>/.claude.json`
/// where the Codemux app has run (see [`desktop_has_run`]). The file itself
/// is never deleted.
pub fn clean_claude_json(home: &Path, config_dir: Option<&Path>) -> Outcome {
    if !desktop_has_run(config_dir) {
        return Outcome::Untouched;
    }
    let path = home.join(".claude.json");
    edit(&path, false, |servers| match servers.get(SERVER) {
        Some(entry) if is_registered_remote_entry(entry) => Ok(true),
        Some(entry) if entry.get("command").and_then(Value::as_str).is_some_and(|c| basename(c) == "codemux-remote") => {
            Err(Outcome::Ambiguous("a codemux-remote entry with other args or settings; it may be yours".into()))
        }
        _ => Ok(false),
    })
}

/// Reads `path`, asks `decide` whether to remove `mcpServers.codemux`, and
/// writes the result back only when it changed and the file did not change
/// meanwhile.
fn edit(
    path: &Path,
    may_delete: bool,
    decide: impl Fn(&Map<String, Value>) -> Result<bool, Outcome>,
) -> Outcome {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Outcome::Untouched,
        Err(error) => return Outcome::Skipped(format!("unreadable: {error}")),
        Ok(metadata) if metadata.file_type().is_symlink() => return Outcome::Skipped("a symlink".into()),
        Ok(metadata) if !metadata.is_file() => return Outcome::Skipped("not a file".into()),
        Ok(_) => {}
    }
    let original = match crate::config_file::read_existing(path) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Outcome::Untouched,
        Err(error) => return Outcome::Skipped(format!("unreadable: {error}")),
    };
    let mut root: Map<String, Value> = match serde_json::from_slice(&original) {
        Ok(Value::Object(root)) => root,
        Ok(_) => return Outcome::Skipped("the top level is not a JSON object".into()),
        Err(error) => return Outcome::Skipped(format!("invalid JSON: {error}")),
    };
    let Some(Value::Object(servers)) = root.get_mut("mcpServers") else {
        return Outcome::Untouched;
    };
    match decide(servers) {
        Ok(true) => {}
        Ok(false) => return Outcome::Untouched,
        Err(outcome) => return outcome,
    }
    servers.remove(SERVER);
    if servers.is_empty() {
        root.remove("mcpServers");
    }
    let written = if root.is_empty() && may_delete {
        crate::config_file::remove_if_unchanged(path, &original).map(|()| Outcome::Deleted)
    } else {
        let mut bytes = serde_json::to_vec_pretty(&Value::Object(root)).expect("JSON values serialize");
        bytes.push(b'\n');
        crate::config_file::replace_if_unchanged(path, Some(&original), &bytes).map(|()| Outcome::Removed)
    };
    written.unwrap_or_else(|error| Outcome::Skipped(error.to_string()))
}

/// Every folder whose `.mcp.json` an older Codemux may have written to:
/// HOME, each workspace folder and its ancestors up to HOME, and the
/// worktrees under Codemux's worktree roots. Sorted and deduplicated.
pub fn project_dirs(home: &Path, workspace_dirs: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut dirs = BTreeSet::from([home.to_path_buf()]);
    for dir in workspace_dirs {
        dirs.extend(launch_dirs(home, &dir));
    }
    let mut budget = MAX_SCANNED_DIRS;
    for root in WORKTREE_ROOTS {
        for repo in child_dirs(&home.join(root), &mut budget) {
            dirs.extend(child_dirs(&repo, &mut budget));
            dirs.insert(repo);
        }
    }
    dirs.into_iter().collect()
}

/// The folders whose `.mcp.json` a Claude started in `dir` reads: `dir` and
/// its parents up to HOME, or only `dir` outside HOME. None for a relative
/// path.
fn launch_dirs(home: &Path, dir: &Path) -> Vec<PathBuf> {
    if !dir.is_absolute() {
        Vec::new()
    } else if dir.starts_with(home) {
        // Claude Code reads `.mcp.json` from every parent folder.
        dir.ancestors().take_while(|ancestor| ancestor.starts_with(home)).map(Path::to_path_buf).collect()
    } else {
        vec![dir.to_path_buf()]
    }
}

/// Real (non-symlink) subdirectories of `dir`, while `budget` lasts.
fn child_dirs(dir: &Path, budget: &mut usize) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        if *budget == 0 {
            log::warn!("[codemux::mcp_cleanup] stopped scanning {} early; the rest is checked next start", dir.display());
            break;
        }
        *budget -= 1;
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            out.push(entry.path());
        }
    }
    out
}

/// Totals of one run, for the log.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub cleaned: Vec<PathBuf>,
    pub ambiguous: Vec<(PathBuf, String)>,
    pub skipped: Vec<(PathBuf, String)>,
}

impl Report {
    fn record(&mut self, path: PathBuf, outcome: Outcome) {
        match outcome {
            Outcome::Untouched => {}
            Outcome::Removed | Outcome::Deleted => self.cleaned.push(path),
            Outcome::Ambiguous(reason) => self.ambiguous.push((path, reason)),
            Outcome::Skipped(reason) => self.skipped.push((path, reason)),
        }
    }

    fn log(&self) {
        for path in &self.cleaned {
            log::info!("[codemux::mcp_cleanup] removed an old Codemux MCP entry from {}", path.display());
        }
        for (path, reason) in &self.ambiguous {
            log::info!("[codemux::mcp_cleanup] left the `codemux` MCP entry in {} alone: {reason}", path.display());
        }
        for (path, reason) in &self.skipped {
            log::debug!("[codemux::mcp_cleanup] skipped {}: {reason}", path.display());
        }
    }
}

/// One full run against `home`, this user's config dir and the given
/// workspace folders.
pub fn run(home: &Path, config_dir: Option<&Path>, workspace_dirs: impl IntoIterator<Item = PathBuf>) -> Report {
    let mut report = Report::default();
    for dir in project_dirs(home, workspace_dirs) {
        let outcome = clean_project_file(&dir);
        report.record(dir.join(".mcp.json"), outcome);
    }
    report.record(home.join(".claude.json"), clean_claude_json(home, config_dir));
    report
}

/// Runs the cleanup on a blocking thread, off the boot path, against this
/// user's HOME and every local folder the app knows (open and archived).
pub fn schedule<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    use tauri::Manager;
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let state = app.state::<crate::state::AppStateStore>();
    let mut folders = Vec::new();
    for workspace in state.snapshot().workspaces.iter().filter(|workspace| !workspace.is_remote_only()) {
        folders.push(workspace.cwd.clone());
        folders.extend(workspace.worktree_path.clone());
        folders.extend(workspace.project_root.clone());
    }
    for archived in state.archived_workspaces_list() {
        folders.push(archived.cwd);
        folders.extend(archived.worktree_path);
        folders.extend(archived.project_root);
    }
    tauri::async_runtime::spawn_blocking(move || {
        run(&home, dirs::config_dir().as_deref(), folders.into_iter().map(PathBuf::from)).log();
    });
}

/// Cleans the folders a Claude started in `dir` reads, before Codemux starts
/// a terminal or chat there. The startup run covers only the folders known
/// at start; a workspace opened, created, imported or restored later may sit
/// below a folder an older build wrote into. Each folder is checked once per
/// process, so later launches cost a set lookup.
pub fn before_launch(dir: &Path) {
    use std::sync::{Mutex, OnceLock};
    static CHECKED: OnceLock<Mutex<BTreeSet<PathBuf>>> = OnceLock::new();
    // Tests clean only inside an isolated root, never the developer's HOME.
    #[cfg(test)]
    if crate::test_isolation::isolated_root().is_none() {
        return;
    }
    let Some(home) = dirs::home_dir() else {
        return;
    };
    before_launch_in(&home, CHECKED.get_or_init(Default::default), dir).log();
}

fn before_launch_in(home: &Path, checked: &std::sync::Mutex<BTreeSet<PathBuf>>, dir: &Path) -> Report {
    let pending: Vec<PathBuf> = {
        let checked = checked.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        launch_dirs(home, dir).into_iter().filter(|dir| !checked.contains(dir)).collect()
    };
    let mut report = Report::default();
    for dir in pending {
        report.record(dir.join(".mcp.json"), clean_project_file(&dir));
        checked.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(dir);
    }
    report
}

/// `codemux-remote serve`'s start: withdraws the registration its older
/// versions wrote into this host's `~/.claude.json`, where the Codemux app
/// has run too. Returns what it did, for the daemon's own log.
pub fn withdraw_remote_registration(home: &Path) -> Outcome {
    clean_claude_json(home, dirs::config_dir().as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const PROVIDER_ORIGIN: &str = "CODEMUX_MCP_PROVIDER_ORIGIN";

    fn write(path: &Path, value: &Value) -> Vec<u8> {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let bytes = serde_json::to_vec_pretty(value).unwrap();
        std::fs::write(path, &bytes).unwrap();
        bytes
    }

    #[test]
    fn only_the_generated_shapes_are_owned() {
        let owned = [
            json!({"command": "/usr/bin/codemux", "args": ["mcp"], "env": {PROVIDER_ORIGIN: "1"}}),
            json!({"command": "/home/u/Apps/Codemux-0.20.AppImage", "args": ["mcp"], "env": {PROVIDER_ORIGIN: "1"}}),
            json!({"command": "/w/target/debug/codemux", "args": ["mcp"], "env": {"CODEMUX_WORKSPACE_ID": "workspace-3"}, "type": "stdio"}),
            json!({"command": "/w/target/debug/deps/codemux_lib-a168139617a00a39", "args": ["mcp"], "env": {PROVIDER_ORIGIN: "1"}}),
            json!({"command": "C:\\Program Files\\Codemux\\codemux.exe", "args": ["mcp"], "env": {PROVIDER_ORIGIN: "1"}}),
            json!({"command": "/home/u/.local/bin/codemux-remote", "args": ["mcp"], "env": {PROVIDER_ORIGIN: "1", "CODEMUX_WORKSPACE_ID": "w"}}),
        ];
        for entry in &owned {
            assert!(is_generated_project_entry(entry), "{entry}");
        }
        let kept = [
            // A bare entry is indistinguishable from one a user added.
            json!({"command": "/usr/bin/codemux", "args": ["mcp"]}),
            json!({"command": "/usr/bin/codemux", "args": ["mcp"], "env": {}}),
            json!({"command": "/usr/bin/codemux", "args": ["mcp", "--client", "claude-code"], "env": {PROVIDER_ORIGIN: "1"}}),
            json!({"command": "/usr/bin/codemux", "args": ["--client", "x", "mcp"], "env": {PROVIDER_ORIGIN: "1"}}),
            json!({"command": "/usr/bin/codemux", "args": ["mcp"], "env": {PROVIDER_ORIGIN: "1", "FOO": "bar"}}),
            json!({"command": "/usr/bin/codemux", "args": ["mcp"], "env": {PROVIDER_ORIGIN: "yes"}}),
            json!({"command": "/usr/bin/codemux", "args": ["mcp"], "env": {"CODEMUX_WORKSPACE_ID": ""}}),
            json!({"command": "/usr/bin/codemux", "args": ["mcp"], "env": {PROVIDER_ORIGIN: "1"}, "cwd": "/x"}),
            json!({"type": "http", "url": "http://127.0.0.1:1/mcp", "env": {PROVIDER_ORIGIN: "1"}}),
            json!({"command": "/usr/bin/codemux", "args": ["mcp"], "env": {PROVIDER_ORIGIN: "1"}, "type": "sse"}),
            // A marker never identifies another program.
            json!({"command": "/usr/bin/my-wrapper", "args": ["mcp"], "env": {PROVIDER_ORIGIN: "1"}}),
            json!({"command": "npx", "args": ["mcp"], "env": {PROVIDER_ORIGIN: "1"}}),
            json!("codemux"),
        ];
        for entry in &kept {
            assert!(!is_generated_project_entry(entry), "{entry}");
        }
    }

    #[test]
    fn project_file_keeps_everything_but_the_generated_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        write(&path, &json!({
            "mcpServers": {
                "codemux": {"command": "/usr/bin/codemux", "args": ["mcp"], "env": {PROVIDER_ORIGIN: "1"}},
                "shadcn": {"command": "npx", "args": ["shadcn@latest", "mcp"]}
            },
            "note": "kept"
        }));
        assert_eq!(clean_project_file(dir.path()), Outcome::Removed);
        let after: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(after, json!({"mcpServers": {"shadcn": {"command": "npx", "args": ["shadcn@latest", "mcp"]}}, "note": "kept"}));

        // Idempotent: the second run changes nothing, byte for byte.
        let bytes = std::fs::read(&path).unwrap();
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(clean_project_file(dir.path()), Outcome::Untouched);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), mtime);
    }

    #[test]
    fn a_file_holding_only_the_generated_entry_is_deleted() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join(".mcp.json"), &json!({"mcpServers": {"codemux": {"command": "/usr/bin/codemux", "args": ["mcp"], "env": {PROVIDER_ORIGIN: "1"}}}}));
        assert_eq!(clean_project_file(dir.path()), Outcome::Deleted);
        assert!(!dir.path().join(".mcp.json").exists());
        assert_eq!(clean_project_file(dir.path()), Outcome::Untouched);

        // Other top-level settings keep the file, without an empty container.
        write(&dir.path().join(".mcp.json"), &json!({"mcpServers": {"codemux": {"command": "/usr/bin/codemux", "args": ["mcp"], "env": {PROVIDER_ORIGIN: "1"}}}, "x": 1}));
        assert_eq!(clean_project_file(dir.path()), Outcome::Removed);
        assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(dir.path().join(".mcp.json")).unwrap()).unwrap(), json!({"x": 1}));
    }

    #[test]
    fn user_authored_and_unreadable_files_stay_byte_identical() {
        let cases: Vec<(&str, Vec<u8>, fn(&Outcome) -> bool)> = vec![
            ("bare", serde_json::to_vec_pretty(&json!({"mcpServers": {"codemux": {"command": "codemux", "args": ["mcp"]}}})).unwrap(), |o| matches!(o, Outcome::Ambiguous(_))),
            ("client", serde_json::to_vec_pretty(&json!({"mcpServers": {"codemux": {"command": "codemux", "args": ["mcp", "--client", "me"]}}})).unwrap(), |o| matches!(o, Outcome::Ambiguous(_))),
            ("foreign", serde_json::to_vec_pretty(&json!({"mcpServers": {"codemux": {"command": "/opt/other", "args": ["mcp"], "env": {PROVIDER_ORIGIN: "1"}}}})).unwrap(), |o| *o == Outcome::Untouched),
            ("other-name", serde_json::to_vec_pretty(&json!({"mcpServers": {"mine": {"command": "/usr/bin/codemux", "args": ["mcp"], "env": {PROVIDER_ORIGIN: "1"}}}})).unwrap(), |o| *o == Outcome::Untouched),
            ("invalid", b"{ \"mcpServers\": { \"codemux\": ".to_vec(), |o| matches!(o, Outcome::Skipped(_))),
            ("array", b"[]".to_vec(), |o| matches!(o, Outcome::Skipped(_))),
            ("servers-array", br#"{"mcpServers": []}"#.to_vec(), |o| *o == Outcome::Untouched),
        ];
        for (name, bytes, expected) in cases {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join(".mcp.json"), &bytes).unwrap();
            let outcome = clean_project_file(dir.path());
            assert!(expected(&outcome), "{name}: {outcome:?}");
            assert_eq!(std::fs::read(dir.path().join(".mcp.json")).unwrap(), bytes, "{name}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_file_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.json");
        let bytes = write(&target, &json!({"mcpServers": {"codemux": {"command": "/usr/bin/codemux", "args": ["mcp"], "env": {PROVIDER_ORIGIN: "1"}}}}));
        std::os::unix::fs::symlink(&target, dir.path().join(".mcp.json")).unwrap();
        assert_eq!(clean_project_file(dir.path()), Outcome::Skipped("a symlink".into()));
        assert_eq!(std::fs::read(&target).unwrap(), bytes);
    }

    /// A config dir where the Codemux app has run (its layout exists).
    fn desktop_config() -> tempfile::TempDir {
        let config = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(config.path().join("codemux-dev")).unwrap();
        std::fs::write(config.path().join("codemux-dev/layout.json"), "{}").unwrap();
        config
    }

    #[test]
    fn claude_json_loses_only_the_registered_remote_entry() {
        let home = tempfile::tempdir().unwrap();
        let config = desktop_config();
        let config = Some(config.path());
        let path = home.path().join(".claude.json");
        write(&path, &json!({
            "numStartups": 7,
            "mcpServers": {
                "codemux": {"command": "/home/u/.local/bin/codemux-remote", "args": ["mcp"]},
                "user": {"command": "y"}
            },
            "projects": {"/p": {"mcpServers": {"codemux": {"command": "/usr/bin/codemux", "args": ["mcp"]}}}}
        }));
        assert_eq!(clean_claude_json(home.path(), config), Outcome::Removed);
        let after: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(after["mcpServers"], json!({"user": {"command": "y"}}));
        assert_eq!(after["numStartups"], 7);
        assert_eq!(after["projects"]["/p"]["mcpServers"]["codemux"]["command"], "/usr/bin/codemux", "never touched");
        assert_eq!(clean_claude_json(home.path(), config), Outcome::Untouched);

        // The file itself is never deleted, even when nothing else is left.
        write(&path, &json!({"mcpServers": {"codemux": {"command": "/x/codemux-remote", "args": ["mcp"]}}}));
        assert_eq!(clean_claude_json(home.path(), config), Outcome::Removed);
        assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), "{}");
    }

    #[test]
    fn claude_json_keeps_every_other_codemux_entry() {
        let config = desktop_config();
        for entry in [
            // What `claude mcp add --scope user codemux -- <codemux-remote> mcp` writes.
            json!({"type": "stdio", "command": "/home/u/.local/bin/codemux-remote", "args": ["mcp"], "env": {}}),
            json!({"command": "/x/codemux-remote", "args": ["mcp"], "type": "stdio"}),
            json!({"command": "/x/codemux-remote", "args": ["mcp"], "env": {}}),
            json!({"command": "/usr/bin/codemux", "args": ["mcp", "--client", "claude-code"]}),
            json!({"command": "/usr/bin/codemux", "args": ["mcp"]}),
            json!({"command": "/x/codemux-remote", "args": ["mcp", "--state-dir", "/s"]}),
            json!({"command": "/x/codemux-remote", "args": ["mcp"], "env": {"A": "1"}}),
            json!({"command": "/x/codemux-remote", "args": ["mcp"], "timeout": 5}),
        ] {
            let home = tempfile::tempdir().unwrap();
            let bytes = write(&home.path().join(".claude.json"), &json!({"mcpServers": {"codemux": entry}}));
            let outcome = clean_claude_json(home.path(), Some(config.path()));
            assert!(matches!(outcome, Outcome::Untouched | Outcome::Ambiguous(_)), "{outcome:?}");
            assert_eq!(std::fs::read(home.path().join(".claude.json")).unwrap(), bytes);
        }
    }

    /// On a host where only `codemux-remote` runs, the user-level `codemux`
    /// entry is how the user reaches it, even in the exact shape older
    /// versions registered, so it is never withdrawn there.
    #[test]
    fn claude_json_is_left_alone_where_the_app_never_ran() {
        let home = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        // codemux-remote's scheduler creates the database there, never a layout.
        std::fs::create_dir_all(config.path().join("codemux")).unwrap();
        std::fs::write(config.path().join("codemux/codemux.db"), "").unwrap();
        let bytes = write(&home.path().join(".claude.json"), &json!({"mcpServers": {"codemux": {"command": "/h/.local/bin/codemux-remote", "args": ["mcp"]}}}));
        for config in [None, Some(config.path())] {
            assert_eq!(clean_claude_json(home.path(), config), Outcome::Untouched);
            assert_eq!(std::fs::read(home.path().join(".claude.json")).unwrap(), bytes);
        }
        assert!(!desktop_has_run(Some(config.path())));
        assert!(desktop_has_run(Some(desktop_config().path())));
    }

    #[test]
    fn targets_cover_home_ancestors_and_worktree_roots() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let project = home.join("Projects/app");
        let worktree = home.join(".codemux/worktrees/app/feature");
        let tmp_fixture = home.join(".codemux/worktrees/.tmpAbc/lower-mode-fixture");
        for dir in [&project, &worktree, &tmp_fixture] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let outside = PathBuf::from("/srv/elsewhere/repo");
        let dirs = project_dirs(home, [project.clone(), outside.clone(), PathBuf::from("relative/dir")]);
        for expected in [home.to_path_buf(), home.join("Projects"), project, outside, worktree, tmp_fixture, home.join(".codemux/worktrees/app")] {
            assert!(dirs.contains(&expected), "{expected:?} missing from {dirs:?}");
        }
        assert!(!dirs.iter().any(|dir| dir == Path::new("/srv") || dir == Path::new("/srv/elsewhere")), "no ancestors outside HOME");
        assert!(!dirs.iter().any(|dir| dir.is_relative()));
    }

    /// A folder no startup run knew about: launching in it cleans the
    /// generated entry in its parent before the launch, keeps everything
    /// else, and checks each folder once.
    #[test]
    fn a_launch_cleans_the_folders_above_it_once() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let generated = json!({"command": "/old/target/debug/codemux", "args": ["mcp"], "env": {PROVIDER_ORIGIN: "1"}});
        write(&home.join("Projects/old/.mcp.json"), &json!({"mcpServers": {"codemux": generated.clone()}}));
        let mine = write(&home.join("Projects/old/repo/.mcp.json"), &json!({"mcpServers": {"codemux": {"command": "codemux", "args": ["mcp", "--client", "me"]}}}));
        let checked = std::sync::Mutex::default();

        let report = before_launch_in(home, &checked, &home.join("Projects/old/repo"));
        assert_eq!(report.cleaned, [home.join("Projects/old/.mcp.json")], "{report:?}");
        assert!(!home.join("Projects/old/.mcp.json").exists());
        assert_eq!(std::fs::read(home.join("Projects/old/repo/.mcp.json")).unwrap(), mine);
        assert_eq!(report.ambiguous.len(), 1);

        // Checked folders are not read again this process; a sibling checks
        // only its own folder.
        write(&home.join("Projects/old/.mcp.json"), &json!({"mcpServers": {"codemux": generated}}));
        assert_eq!(before_launch_in(home, &checked, &home.join("Projects/old/repo")), Report::default());
        std::fs::create_dir_all(home.join("Projects/old/other")).unwrap();
        assert_eq!(before_launch_in(home, &checked, &home.join("Projects/old/other")), Report::default());
        assert_eq!(checked.lock().unwrap().len(), 5, "HOME, Projects, old, repo, other");
    }

    /// The incident's layout: a generated entry in HOME and a project, a
    /// user's own `--client` entry, and Vexis's YAML. One run removes only
    /// what Codemux wrote; a second run changes nothing.
    #[test]
    fn a_full_run_cleans_owned_entries_and_leaves_the_rest() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        let generated = json!({"command": "/w/target/debug/codemux", "args": ["mcp"], "env": {PROVIDER_ORIGIN: "1"}});
        write(&home.join(".mcp.json"), &json!({"mcpServers": {"codemux": generated.clone()}}));
        write(&home.join("Projects/p/.mcp.json"), &json!({"mcpServers": {"codemux": generated, "db": {"command": "pg"}}}));
        let mine = write(&home.join("Projects/mine/.mcp.json"), &json!({"mcpServers": {"codemux": {"command": "codemux", "args": ["mcp", "--client", "me"]}}}));
        let vexis_yaml = b"# mine\nservers:\n  - name: codemux\n    command: /home/u/.local/bin/codemux-remote\n    args: [\"mcp\"]\n";
        std::fs::create_dir_all(home.join(".vexis")).unwrap();
        std::fs::write(home.join(".vexis/mcp-servers.yaml"), vexis_yaml).unwrap();
        write(&home.join(".claude.json"), &json!({"mcpServers": {"codemux": {"command": "/h/.local/bin/codemux-remote", "args": ["mcp"]}}, "k": 1}));

        let config = desktop_config();
        let report = run(home, Some(config.path()), [home.join("Projects/p"), home.join("Projects/mine")]);
        assert_eq!(report.cleaned.len(), 3, "{report:?}");
        assert!(!home.join(".mcp.json").exists());
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(home.join("Projects/p/.mcp.json")).unwrap()).unwrap(),
            json!({"mcpServers": {"db": {"command": "pg"}}})
        );
        assert_eq!(std::fs::read(home.join("Projects/mine/.mcp.json")).unwrap(), mine);
        assert_eq!(report.ambiguous.len(), 1);
        assert_eq!(std::fs::read(home.join(".vexis/mcp-servers.yaml")).unwrap(), vexis_yaml, "Vexis is never touched");
        assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(home.join(".claude.json")).unwrap()).unwrap(), json!({"k": 1}));

        let again = run(home, Some(config.path()), [home.join("Projects/p"), home.join("Projects/mine")]);
        assert!(again.cleaned.is_empty() && again.skipped.is_empty(), "{again:?}");
    }
}
