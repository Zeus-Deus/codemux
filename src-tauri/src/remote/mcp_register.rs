//! Register `codemux-remote mcp` as the user-level `codemux` MCP server
//! in known agent configs on a host, so an agent CLI the user runs there
//! can manage the host's folders, worktrees and shells without the user
//! editing config files by hand.
//!
//! Why this exists: the desktop push flow drops a per-workspace
//! `.mcp.json` (handled by `ssh::bootstrap::provision_workspace_mcp_config`).
//! But users also work on directories on the host that weren't
//! pushed — repos cloned directly on the server, ad-hoc scratch
//! dirs, services running in arbitrary places. For those, agents
//! need a user-level (not per-workspace) MCP config that names
//! `codemux-remote` as a server. `codemux-remote serve` runs this on
//! every start.
//!
//! Who owns the `codemux` name: where the Codemux app (desktop or
//! `codemux serve`) has run as this user, `codemux` means the app's
//! `codemux mcp`, which drives chats. codemux-remote only manages this
//! host, so there it adds nothing and withdraws the entry it wrote
//! before. Everywhere else it registers itself.
//!
//! Safety contract:
//!
//! 1. **Ownership.** Only a missing entry or one that runs codemux-remote
//!    (judged by the command's file name) is written. An entry that runs
//!    any other program (the app's `codemux`, an AppImage, a wrapper, a
//!    URL server) is left exactly as it is. A current entry is not
//!    rewritten, so running on every daemon start is fine.
//! 2. **Atomic.** Writes go through a sibling `.tmp` file + rename
//!    so a crash mid-write can never leave the user's agent config
//!    in a half-baked state. Claude rewrites `~/.claude.json` without
//!    a lock, so the file is read again just before the rename and the
//!    update starts over on what was saved if it changed. A symlinked
//!    config is written through to the file it names.
//! 3. **No corruption.** If the existing file is unparseable
//!    (broken JSON/YAML), or its server list has an unexpected shape,
//!    we log and bail — never overwrite a file we can't safely
//!    round-trip.
//! 4. **Skip missing tools.** A user who doesn't have Claude Code
//!    won't have `~/.claude.json`. We skip configs whose parent
//!    directory doesn't exist. We don't create agent-specific
//!    directories the user never opted into.
//! 5. **Withdraws its own entry where the Codemux app has run.** See
//!    [`name_owner`]; only the `codemux` key is managed, so a user who
//!    wants both can add codemux-remote under another name.
//!
//! Supported agent configs (v1):
//!
//! | Agent | Path | Format |
//! |---|---|---|
//! | Claude Code | `~/.claude.json` | JSON |
//! | Vexis | `~/.vexis/mcp-servers.yaml` | YAML |
//!
//! Future targets (Codex, Gemini CLI, OpenCode): each has its own
//! config path/shape; add them here as users ask. Until then, the
//! website docs cover the one-line manual edit.

#![cfg(unix)]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde_json::{json, Value as JsonValue};

/// Who may own the user-level `codemux` MCP entry on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameOwner {
    /// codemux-remote is the only Codemux here, so it registers itself as `codemux`.
    CodemuxRemote,
    /// The Codemux app (desktop or `codemux serve`) has run here as this user.
    /// Its `codemux mcp` owns the name: codemux-remote adds nothing and
    /// withdraws an entry it wrote before.
    CodemuxApp,
}

/// The app persists its window layout at `<config>/codemux/layout.json`
/// (`codemux-dev` for debug builds). codemux-remote never writes that file;
/// its scheduler does create `codemux.db` there, so the database is not proof.
pub fn name_owner(config_dir: Option<&Path>) -> NameOwner {
    // Both names whichever build this is: a release helper must see a dev
    // desktop's layout, and a dev helper a release desktop's.
    const APP_CONFIG_DIRS: [&str; 2] = ["codemux", "codemux-dev"];
    let app_has_run = config_dir.is_some_and(|dir| {
        APP_CONFIG_DIRS
            .iter()
            .any(|name| dir.join(name).join("layout.json").is_file())
    });
    if app_has_run {
        NameOwner::CodemuxApp
    } else {
        NameOwner::CodemuxRemote
    }
}

/// What happened to one config file's `codemux` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryOutcome {
    Written,
    Unchanged,
    Withdrawn,
    /// Another program's entry; carries its command ("" when it has none).
    Foreign(String),
    Absent,
}

/// True when an entry starts codemux-remote, judged by file name.
pub fn is_codemux_remote_command(command: Option<&str>) -> bool {
    command
        .map(Path::new)
        .and_then(Path::file_name)
        .is_some_and(|name| name == OsStr::new("codemux-remote"))
}

/// Outcome of scanning + updating known agent configs.
#[derive(Debug, Default)]
pub struct RegisterReport {
    /// Paths whose `codemux` entry we added or refreshed.
    pub modified: Vec<PathBuf>,
    /// Paths whose codemux-remote entry we withdrew because the app
    /// owns the name here.
    pub removed: Vec<PathBuf>,
    /// Paths we examined that needed no change: a current entry, or
    /// no entry where the app owns the name.
    pub unchanged: Vec<PathBuf>,
    /// Paths whose `codemux` entry runs another program, with its
    /// command. Left alone.
    pub foreign: Vec<(PathBuf, String)>,
    /// Paths we wanted to touch but couldn't (malformed file, IO
    /// error). Each entry includes the reason for diagnostics.
    pub failed: Vec<(PathBuf, String)>,
}

impl RegisterReport {
    fn record(&mut self, path: PathBuf, result: Result<EntryOutcome, String>) {
        match result {
            Ok(EntryOutcome::Written) => self.modified.push(path),
            Ok(EntryOutcome::Withdrawn) => self.removed.push(path),
            Ok(EntryOutcome::Unchanged | EntryOutcome::Absent) => self.unchanged.push(path),
            Ok(EntryOutcome::Foreign(command)) => self.foreign.push((path, command)),
            Err(reason) => self.failed.push((path, reason)),
        }
    }
}

/// Probe every known agent-config location and make each one's
/// `codemux` MCP server entry follow the ownership rule for
/// `codemux_remote_path` (typically `std::env::current_exe()`).
///
/// Best-effort: returns a [`RegisterReport`] with per-path outcomes,
/// never an `Err`. Caller logs whatever it likes.
pub fn ensure_codemux_in_agent_configs(codemux_remote_path: &Path) -> RegisterReport {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return RegisterReport::default(),
    };
    let owner = name_owner(dirs::config_dir().as_deref());
    ensure_codemux_in_agent_configs_with_home(codemux_remote_path, &home, owner)
}

/// Test-friendly variant of [`ensure_codemux_in_agent_configs`] that
/// takes an explicit `home` directory and name owner. Production
/// callers use the `dirs`-rooted wrapper above; this form lets tests
/// target a tempdir without mutating the process's `HOME` env var
/// (which would race other tests).
pub fn ensure_codemux_in_agent_configs_with_home(
    codemux_remote_path: &Path,
    home: &Path,
    owner: NameOwner,
) -> RegisterReport {
    let mut report = RegisterReport::default();
    let exec = codemux_remote_path.to_string_lossy().into_owned();

    // Claude Code: ~/.claude.json
    let claude = home.join(".claude.json");
    if claude.exists() {
        let result = ensure_in_json_config(&claude, &exec, owner);
        report.record(claude, result);
    }

    // Vexis: ~/.vexis/mcp-servers.yaml
    let vexis_dir = home.join(".vexis");
    if vexis_dir.is_dir() {
        let vexis = vexis_dir.join("mcp-servers.yaml");
        let result = ensure_in_yaml_config(&vexis, &exec, owner);
        report.record(vexis, result);
    }

    report
}

/// The `codemux` entry a config file holds today, as the ownership rule sees it.
enum Existing<'a> {
    Missing,
    Ours { current: bool },
    Foreign(&'a str),
}

/// What to do with the entry.
enum Change {
    Write,
    Withdraw,
    Keep(EntryOutcome),
}

/// The ownership table: codemux-remote writes a missing or stale entry of
/// its own where it owns the name, withdraws its own entry where the app
/// does, and never touches another program's entry.
fn decide(existing: Existing<'_>, owner: NameOwner) -> Change {
    match (owner, existing) {
        (_, Existing::Foreign(command)) => Change::Keep(EntryOutcome::Foreign(command.to_string())),
        (NameOwner::CodemuxRemote, Existing::Missing) => Change::Write,
        (NameOwner::CodemuxRemote, Existing::Ours { current: true }) => {
            Change::Keep(EntryOutcome::Unchanged)
        }
        (NameOwner::CodemuxRemote, Existing::Ours { current: false }) => Change::Write,
        (NameOwner::CodemuxApp, Existing::Missing) => Change::Keep(EntryOutcome::Absent),
        (NameOwner::CodemuxApp, Existing::Ours { .. }) => Change::Withdraw,
    }
}

/// An existing entry from its command and whether its command and args are
/// already what we would write. Other keys (`type`, `env`) don't matter:
/// Claude adds `"type":"stdio","env":{}` when it normalises an entry.
fn classify(command: Option<&str>, current: bool) -> Existing<'_> {
    if is_codemux_remote_command(command) {
        Existing::Ours { current }
    } else {
        Existing::Foreign(command.unwrap_or(""))
    }
}

/// A planned update: the outcome, and the new file bytes when it writes.
type Plan = (EntryOutcome, Option<Vec<u8>>);

/// JSON-shaped config writer. Used for `~/.claude.json` and any
/// `.mcp.json`-style file. The file must exist.
///
/// The wire shape is:
///
/// ```json
/// {
///   "mcpServers": {
///     "codemux": { "command": "<path>", "args": ["mcp"] }
///   }
/// }
/// ```
///
/// We never touch keys other than `mcpServers.codemux`. Any other
/// MCP servers the user has configured, any other top-level keys,
/// stay untouched. If `mcpServers` isn't present we create it when
/// we write.
pub fn ensure_in_json_config(
    path: &Path,
    codemux_remote_path: &str,
    owner: NameOwner,
) -> Result<EntryOutcome, String> {
    update_config_file(path, |bytes| plan_json(bytes, codemux_remote_path, owner))
}

fn plan_json(bytes: Option<&[u8]>, exe: &str, owner: NameOwner) -> Result<Plan, String> {
    let bytes = bytes.ok_or_else(|| "read: the file does not exist".to_string())?;
    let mut config: JsonValue =
        serde_json::from_slice(bytes).map_err(|e| format!("parse JSON: {e}"))?;

    // If the file isn't an object at the top level we bail rather
    // than overwrite — could be a deliberately exotic config shape.
    let root = config
        .as_object_mut()
        .ok_or_else(|| "top-level value is not a JSON object".to_string())?;
    let servers = match root.get("mcpServers") {
        None => None,
        Some(JsonValue::Object(servers)) => Some(servers),
        Some(_) => return Err("mcpServers is not an object".into()),
    };
    let entry = servers.and_then(|servers| servers.get("codemux"));
    let existing = match entry {
        None => Existing::Missing,
        Some(entry) => {
            let command = entry.get("command").and_then(JsonValue::as_str);
            let current = command == Some(exe) && entry.get("args") == Some(&json!(["mcp"]));
            classify(command, current)
        }
    };

    let outcome = match decide(existing, owner) {
        Change::Keep(outcome) => return Ok((outcome, None)),
        Change::Write => {
            let servers = root
                .entry("mcpServers")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .expect("mcpServers checked to be an object");
            match servers.get_mut("codemux").and_then(JsonValue::as_object_mut) {
                // Our own stale entry: refresh what we own, keep the rest.
                Some(entry) => {
                    entry.insert("command".into(), json!(exe));
                    entry.insert("args".into(), json!(["mcp"]));
                }
                None => {
                    servers.insert("codemux".into(), json!({ "command": exe, "args": ["mcp"] }));
                }
            }
            EntryOutcome::Written
        }
        Change::Withdraw => {
            if let Some(JsonValue::Object(servers)) = root.get_mut("mcpServers") {
                servers.remove("codemux");
            }
            EntryOutcome::Withdrawn
        }
    };

    let serialised =
        serde_json::to_string_pretty(&config).map_err(|e| format!("serialise JSON: {e}"))?;
    Ok((outcome, Some(serialised.into_bytes())))
}

/// YAML-shaped config writer for Vexis's `~/.vexis/mcp-servers.yaml`.
///
/// The Vexis schema is:
///
/// ```yaml
/// servers:
///   - name: codemux
///     command: <path>
///     args: ["mcp"]
/// ```
///
/// A vector of server entries keyed by `name`. We manage the entry
/// named `codemux` while preserving every other entry verbatim.
pub fn ensure_in_yaml_config(
    path: &Path,
    codemux_remote_path: &str,
    owner: NameOwner,
) -> Result<EntryOutcome, String> {
    update_config_file(path, |bytes| plan_yaml(bytes, codemux_remote_path, owner))
}

fn plan_yaml(bytes: Option<&[u8]>, exe: &str, owner: NameOwner) -> Result<Plan, String> {
    use serde_yaml::{Mapping, Value as YamlValue};

    // If the file doesn't exist yet, create a minimal one where the name
    // is ours. We do this here (vs. the JSON path's "must exist" rule)
    // because the `.vexis` directory existing means the user definitely
    // uses Vexis, even if they never wrote any MCP servers in before.
    let Some(bytes) = bytes else {
        return Ok(match owner {
            NameOwner::CodemuxRemote => {
                let new_doc = format!(
                    "servers:\n  - name: codemux\n    command: {cmd}\n    args: [\"mcp\"]\n",
                    cmd = yaml_quote_if_needed(exe),
                );
                (EntryOutcome::Written, Some(new_doc.into_bytes()))
            }
            NameOwner::CodemuxApp => (EntryOutcome::Absent, None),
        });
    };

    let mut doc: YamlValue =
        serde_yaml::from_slice(bytes).map_err(|e| format!("parse YAML: {e}"))?;

    // Empty file → top-level becomes Null. Treat as "no servers
    // yet" and seed the structure.
    if doc.is_null() {
        doc = YamlValue::Mapping(Mapping::new());
    }

    let map = doc
        .as_mapping_mut()
        .ok_or_else(|| "top-level YAML is not a mapping".to_string())?;
    let servers_key = YamlValue::String("servers".into());
    let name_key = YamlValue::String("name".into());
    let command_key = YamlValue::String("command".into());
    let args_key = YamlValue::String("args".into());
    let mcp_args = YamlValue::Sequence(vec!["mcp".into()]);

    // A bare `servers:` (null) holds nothing, so it is treated like a
    // missing key; any other non-list shape is refused.
    let servers: &[YamlValue] = match map.get(&servers_key) {
        None | Some(YamlValue::Null) => &[],
        Some(YamlValue::Sequence(servers)) => servers,
        Some(_) => return Err("servers is not a list".into()),
    };
    let index = servers.iter().position(|entry| {
        entry.get(&name_key).and_then(YamlValue::as_str) == Some("codemux")
    });
    let existing = match index.map(|i| &servers[i]) {
        None => Existing::Missing,
        Some(entry) => {
            let command = entry.get(&command_key).and_then(YamlValue::as_str);
            let current = command == Some(exe) && entry.get(&args_key) == Some(&mcp_args);
            classify(command, current)
        }
    };

    let outcome = match decide(existing, owner) {
        Change::Keep(outcome) => return Ok((outcome, None)),
        Change::Write => {
            if !map.get(&servers_key).is_some_and(YamlValue::is_sequence) {
                map.insert(servers_key.clone(), YamlValue::Sequence(vec![]));
            }
            let servers = map
                .get_mut(&servers_key)
                .and_then(YamlValue::as_sequence_mut)
                .expect("servers sequence we just ensured exists");
            match index.and_then(|i| servers[i].as_mapping_mut()) {
                // Our own stale entry: refresh what we own, keep the rest.
                Some(entry) => {
                    entry.insert(command_key, exe.into());
                    entry.insert(args_key, mcp_args);
                }
                None => {
                    let mut desired = Mapping::new();
                    desired.insert(name_key, "codemux".into());
                    desired.insert(command_key, exe.into());
                    desired.insert(args_key, mcp_args);
                    servers.push(YamlValue::Mapping(desired));
                }
            }
            EntryOutcome::Written
        }
        Change::Withdraw => {
            if let (Some(index), Some(servers)) = (
                index,
                map.get_mut(&servers_key).and_then(YamlValue::as_sequence_mut),
            ) {
                servers.remove(index);
            }
            EntryOutcome::Withdrawn
        }
    };

    let serialised = serde_yaml::to_string(&doc).map_err(|e| format!("serialise YAML: {e}"))?;
    Ok((outcome, Some(serialised.into_bytes())))
}

/// Replace `path` with what `plan` makes of its current bytes. Claude
/// rewrites `~/.claude.json` without a lock, so the file is read again just
/// before the rename; if it changed, the plan runs again on what was saved.
/// Writes through a symlink to the file it names.
fn update_config_file(
    path: &Path,
    mut plan: impl FnMut(Option<&[u8]>) -> Result<Plan, String>,
) -> Result<EntryOutcome, String> {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    for _ in 0..3 {
        let before = read_if_present(&path)?;
        let (outcome, next) = plan(before.as_deref())?;
        let Some(next) = next else {
            return Ok(outcome);
        };
        let tmp = write_temp(&path, &next)?;
        if read_if_present(&path)? != before {
            let _ = std::fs::remove_file(&tmp);
            continue;
        }
        std::fs::rename(&tmp, &path).map_err(|e| format!("rename into place: {e}"))?;
        return Ok(outcome);
    }
    Err("the file kept changing while it was updated; will try again on the next start".into())
}

fn read_if_present(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("read: {e}")),
    }
}

/// Write `content` to the sibling `<path>.tmp` and fsync it, ready to be
/// renamed into place. Preserves the existing file's mode if we can;
/// otherwise leaves the default umask.
fn write_temp(path: &Path, content: &[u8]) -> Result<PathBuf, String> {
    use std::io::Write;

    let parent = path
        .parent()
        .ok_or_else(|| "path has no parent directory".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("create parent: {e}"))?;

    let tmp = path.with_extension({
        // Preserve the original extension and append `.tmp` so the
        // tempfile doesn't end up named `foo.tmp` when the source
        // was `foo.json`. Tools that match against extension (e.g.
        // file watchers) won't get confused.
        let original_ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if original_ext.is_empty() {
            "tmp".to_string()
        } else {
            format!("{original_ext}.tmp")
        }
    });

    // Inherit mode from the existing file if any — agent configs
    // are often 0600 (containing API keys) and we don't want to
    // accidentally widen permissions.
    let existing_mode = std::fs::metadata(path).ok().map(|m| {
        use std::os::unix::fs::PermissionsExt;
        m.permissions().mode() & 0o777
    });

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&tmp)
        .map_err(|e| format!("open tmpfile: {e}"))?;
    if let Some(mode) = existing_mode {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(std::fs::Permissions::from_mode(mode));
    }
    file.write_all(content)
        .map_err(|e| format!("write tmpfile: {e}"))?;
    file.sync_all().map_err(|e| format!("fsync tmpfile: {e}"))?;
    Ok(tmp)
}

/// Wrap a string in double-quotes if it contains characters YAML
/// would otherwise interpret (spaces, colons, backslashes). Pure
/// path strings like `/home/user/.local/bin/codemux-remote` work
/// unquoted, but we be paranoid for the case where the path
/// contains spaces.
fn yaml_quote_if_needed(s: &str) -> String {
    if s.chars().any(|c| matches!(c, ' ' | ':' | '\\' | '"' | '\'' | '#')) {
        // Naive quoting: escape backslashes and double-quotes.
        let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const OURS: NameOwner = NameOwner::CodemuxRemote;
    const APP: NameOwner = NameOwner::CodemuxApp;

    fn read_to_string(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    fn json_file(dir: &TempDir, content: &str) -> PathBuf {
        let path = dir.path().join("claude.json");
        std::fs::write(&path, content).unwrap();
        path
    }

    fn yaml_file(dir: &TempDir, content: &str) -> PathBuf {
        let path = dir.path().join("mcp-servers.yaml");
        std::fs::write(&path, content).unwrap();
        path
    }

    fn yaml_names(path: &Path) -> Vec<String> {
        let parsed: serde_yaml::Value = serde_yaml::from_str(&read_to_string(path)).unwrap();
        parsed["servers"]
            .as_sequence()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_string())
            .collect()
    }

    // ─── JSON ──────────────────────────────────────────────────

    #[test]
    fn json_adds_codemux_to_existing_empty_config() {
        let dir = TempDir::new().unwrap();
        let path = json_file(&dir, "{}");

        assert_eq!(
            ensure_in_json_config(&path, "/bin/codemux-remote", OURS).unwrap(),
            EntryOutcome::Written
        );

        let parsed: JsonValue = serde_json::from_str(&read_to_string(&path)).unwrap();
        assert_eq!(parsed["mcpServers"]["codemux"]["command"], json!("/bin/codemux-remote"));
        assert_eq!(parsed["mcpServers"]["codemux"]["args"], json!(["mcp"]));
    }

    #[test]
    fn json_preserves_unrelated_servers() {
        let dir = TempDir::new().unwrap();
        let path = json_file(
            &dir,
            r#"{
                "mcpServers": {
                    "shadcn": { "command": "npx", "args": ["shadcn"] },
                    "github": { "command": "gh-mcp" }
                },
                "otherTopLevel": "stays"
            }"#,
        );

        assert_eq!(
            ensure_in_json_config(&path, "/bin/codemux-remote", OURS).unwrap(),
            EntryOutcome::Written
        );

        let parsed: JsonValue = serde_json::from_str(&read_to_string(&path)).unwrap();
        // Other servers + top-level keys untouched.
        assert_eq!(parsed["mcpServers"]["shadcn"]["command"], json!("npx"));
        assert_eq!(parsed["mcpServers"]["github"]["command"], json!("gh-mcp"));
        assert_eq!(parsed["otherTopLevel"], json!("stays"));
        // Codemux added.
        assert_eq!(parsed["mcpServers"]["codemux"]["command"], json!("/bin/codemux-remote"));
    }

    #[test]
    fn json_is_idempotent_when_already_correct() {
        let dir = TempDir::new().unwrap();
        let path = json_file(
            &dir,
            r#"{"mcpServers":{"codemux":{"command":"/bin/codemux-remote","args":["mcp"]}}}"#,
        );
        let before = read_to_string(&path);

        assert_eq!(
            ensure_in_json_config(&path, "/bin/codemux-remote", OURS).unwrap(),
            EntryOutcome::Unchanged
        );
        assert_eq!(read_to_string(&path), before, "no rewrite when entry is current");
    }

    #[test]
    fn json_updates_when_command_path_changes() {
        let dir = TempDir::new().unwrap();
        let path = json_file(
            &dir,
            r#"{"mcpServers":{"codemux":{"command":"/old/bin/codemux-remote","args":["mcp"]}}}"#,
        );
        assert_eq!(
            ensure_in_json_config(&path, "/new/bin/codemux-remote", OURS).unwrap(),
            EntryOutcome::Written
        );
        let parsed: JsonValue = serde_json::from_str(&read_to_string(&path)).unwrap();
        assert_eq!(parsed["mcpServers"]["codemux"]["command"], json!("/new/bin/codemux-remote"));
    }

    #[test]
    fn json_refuses_to_overwrite_malformed_file() {
        let dir = TempDir::new().unwrap();
        let path = json_file(&dir, "this is not json {{{");
        let err = ensure_in_json_config(&path, "/bin/codemux-remote", OURS).unwrap_err();
        assert!(err.contains("parse JSON"));
        // File untouched.
        assert_eq!(std::fs::read(&path).unwrap(), b"this is not json {{{");
    }

    #[test]
    fn json_refuses_top_level_array() {
        let dir = TempDir::new().unwrap();
        let path = json_file(&dir, "[]");
        let err = ensure_in_json_config(&path, "/bin/codemux-remote", OURS).unwrap_err();
        assert!(err.contains("not a JSON object"));
    }

    #[test]
    fn json_preserves_file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = json_file(&dir, "{}");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        assert_eq!(
            ensure_in_json_config(&path, "/bin/codemux-remote", OURS).unwrap(),
            EntryOutcome::Written
        );
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "must not widen perms on agent config");
    }

    #[test]
    fn json_leaves_a_codemux_entry_it_does_not_own() {
        let foreign = [
            (r#"{"command":"/usr/bin/codemux","args":["mcp"]}"#, "/usr/bin/codemux"),
            (
                r#"{"command":"/home/u/Apps/Codemux.AppImage","args":["mcp"],"env":{"CODEMUX_MCP_PROVIDER_ORIGIN":"1"}}"#,
                "/home/u/Apps/Codemux.AppImage",
            ),
            (r#"{"type":"http","url":"https://example.test/mcp"}"#, ""),
            (r#"{"command":"docker","args":["run","-i","codemux-remote"]}"#, "docker"),
        ];
        for (entry, command) in foreign {
            let dir = TempDir::new().unwrap();
            let content = format!(r#"{{"mcpServers":{{"codemux":{entry}}},"theme":"dark"}}"#);
            let path = json_file(&dir, &content);

            assert_eq!(
                ensure_in_json_config(&path, "/bin/codemux-remote", OURS).unwrap(),
                EntryOutcome::Foreign(command.to_string()),
                "{entry}"
            );
            assert_eq!(read_to_string(&path), content, "bytes unchanged for {entry}");
        }
    }

    #[test]
    fn json_refresh_keeps_the_other_keys_of_its_own_entry() {
        let dir = TempDir::new().unwrap();
        let path = json_file(
            &dir,
            r#"{"mcpServers":{"codemux":{"type":"stdio","command":"/old/codemux-remote","args":["mcp"],"env":{"RUST_LOG":"debug"}}}}"#,
        );

        assert_eq!(
            ensure_in_json_config(&path, "/bin/codemux-remote", OURS).unwrap(),
            EntryOutcome::Written
        );
        let parsed: JsonValue = serde_json::from_str(&read_to_string(&path)).unwrap();
        let entry = &parsed["mcpServers"]["codemux"];
        assert_eq!(entry["command"], json!("/bin/codemux-remote"));
        assert_eq!(entry["args"], json!(["mcp"]));
        assert_eq!(entry["type"], json!("stdio"));
        assert_eq!(entry["env"], json!({ "RUST_LOG": "debug" }));
    }

    #[test]
    fn json_is_current_when_claude_added_type_and_env() {
        let dir = TempDir::new().unwrap();
        let content = r#"{"mcpServers":{"codemux":{"type":"stdio","command":"/bin/codemux-remote","args":["mcp"],"env":{}}}}"#;
        let path = json_file(&dir, content);

        assert_eq!(
            ensure_in_json_config(&path, "/bin/codemux-remote", OURS).unwrap(),
            EntryOutcome::Unchanged
        );
        assert_eq!(read_to_string(&path), content, "no rewrite");
    }

    #[test]
    fn json_withdraws_its_entry_where_the_app_owns_the_name() {
        let dir = TempDir::new().unwrap();
        let path = json_file(
            &dir,
            r#"{"mcpServers":{"codemux":{"command":"/home/u/.local/bin/codemux-remote","args":["mcp"]},"github":{"command":"gh-mcp"}},"otherTopLevel":"stays"}"#,
        );

        assert_eq!(
            ensure_in_json_config(&path, "/home/u/.local/bin/codemux-remote", APP).unwrap(),
            EntryOutcome::Withdrawn
        );
        let parsed: JsonValue = serde_json::from_str(&read_to_string(&path)).unwrap();
        assert!(parsed["mcpServers"].get("codemux").is_none());
        assert_eq!(parsed["mcpServers"]["github"]["command"], json!("gh-mcp"));
        assert_eq!(parsed["otherTopLevel"], json!("stays"));

        // The last server gone still leaves an (empty) mcpServers object.
        let only = json_file(
            &dir,
            r#"{"mcpServers":{"codemux":{"command":"/elsewhere/codemux-remote","args":["mcp"]}}}"#,
        );
        assert_eq!(
            ensure_in_json_config(&only, "/home/u/.local/bin/codemux-remote", APP).unwrap(),
            EntryOutcome::Withdrawn
        );
        let parsed: JsonValue = serde_json::from_str(&read_to_string(&only)).unwrap();
        assert_eq!(parsed["mcpServers"], json!({}));
    }

    #[test]
    fn json_adds_nothing_where_the_app_owns_the_name() {
        let dir = TempDir::new().unwrap();
        for content in ["{}", r#"{"mcpServers":{"github":{"command":"gh-mcp"}}}"#] {
            let path = json_file(&dir, content);
            assert_eq!(
                ensure_in_json_config(&path, "/bin/codemux-remote", APP).unwrap(),
                EntryOutcome::Absent
            );
            assert_eq!(read_to_string(&path), content, "bytes unchanged");
        }
    }

    #[test]
    fn json_keeps_the_apps_entry() {
        let dir = TempDir::new().unwrap();
        let content = r#"{"mcpServers":{"codemux":{"command":"/usr/bin/codemux","args":["mcp"]}}}"#;
        let path = json_file(&dir, content);

        assert_eq!(
            ensure_in_json_config(&path, "/bin/codemux-remote", APP).unwrap(),
            EntryOutcome::Foreign("/usr/bin/codemux".into())
        );
        assert_eq!(read_to_string(&path), content);
    }

    #[test]
    fn json_refuses_a_non_object_mcp_servers() {
        let dir = TempDir::new().unwrap();
        let content = r#"{"mcpServers":[]}"#;
        let path = json_file(&dir, content);

        let err = ensure_in_json_config(&path, "/bin/codemux-remote", OURS).unwrap_err();
        assert!(err.contains("mcpServers is not an object"), "{err}");
        assert_eq!(read_to_string(&path), content);
    }

    #[test]
    fn json_writes_through_a_symlink() {
        let dir = TempDir::new().unwrap();
        let real = dir.path().join("real.json");
        std::fs::write(&real, "{}").unwrap();
        let link = dir.path().join(".claude.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        assert_eq!(
            ensure_in_json_config(&link, "/bin/codemux-remote", OURS).unwrap(),
            EntryOutcome::Written
        );
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        let parsed: JsonValue = serde_json::from_str(&read_to_string(&real)).unwrap();
        assert_eq!(parsed["mcpServers"]["codemux"]["command"], json!("/bin/codemux-remote"));
    }

    #[test]
    fn json_update_starts_over_when_the_file_changes_meanwhile() {
        let dir = TempDir::new().unwrap();
        let path = json_file(&dir, r#"{"numStartups":1}"#);
        let mut first = true;

        let outcome = update_config_file(&path, |bytes| {
            if first {
                // Claude saves between our read and our rename.
                first = false;
                std::fs::write(&path, r#"{"numStartups":2,"savedByClaude":true}"#).unwrap();
            }
            plan_json(bytes, "/bin/codemux-remote", OURS)
        })
        .unwrap();

        assert_eq!(outcome, EntryOutcome::Written);
        let parsed: JsonValue = serde_json::from_str(&read_to_string(&path)).unwrap();
        assert_eq!(parsed["savedByClaude"], json!(true));
        assert_eq!(parsed["numStartups"], json!(2));
        assert_eq!(parsed["mcpServers"]["codemux"]["command"], json!("/bin/codemux-remote"));
        assert!(!path.with_extension("json.tmp").exists(), "no temp file left behind");
    }

    // ─── YAML ──────────────────────────────────────────────────

    #[test]
    fn yaml_creates_missing_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("mcp-servers.yaml");
        assert!(!path.exists());

        assert_eq!(
            ensure_in_yaml_config(&path, "/bin/codemux-remote", OURS).unwrap(),
            EntryOutcome::Written
        );
        assert!(path.exists());
        let parsed: serde_yaml::Value =
            serde_yaml::from_str(&read_to_string(&path)).unwrap();
        let servers = parsed["servers"].as_sequence().unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0]["name"], serde_yaml::Value::from("codemux"));
        assert_eq!(servers[0]["command"], serde_yaml::Value::from("/bin/codemux-remote"));
    }

    #[test]
    fn yaml_preserves_unrelated_servers() {
        let dir = TempDir::new().unwrap();
        let path = yaml_file(
            &dir,
            r#"servers:
  - name: shadcn
    command: npx
    args: ["@shadcn/mcp"]
  - name: github
    command: gh-mcp
"#,
        );
        assert_eq!(
            ensure_in_yaml_config(&path, "/bin/codemux-remote", OURS).unwrap(),
            EntryOutcome::Written
        );

        // Three entries: shadcn, github, codemux.
        assert_eq!(yaml_names(&path), ["shadcn", "github", "codemux"]);
    }

    #[test]
    fn yaml_is_idempotent_when_already_correct() {
        let dir = TempDir::new().unwrap();
        let path = yaml_file(
            &dir,
            "servers:\n  - name: codemux\n    command: /bin/codemux-remote\n    args:\n      - mcp\n",
        );
        let before = read_to_string(&path);

        assert_eq!(
            ensure_in_yaml_config(&path, "/bin/codemux-remote", OURS).unwrap(),
            EntryOutcome::Unchanged
        );
        assert_eq!(read_to_string(&path), before, "no rewrite when entry is current");
    }

    #[test]
    fn yaml_updates_when_command_changes() {
        let dir = TempDir::new().unwrap();
        let path = yaml_file(
            &dir,
            "servers:\n  - name: codemux\n    command: /old/codemux-remote\n    args:\n      - mcp\n",
        );
        assert_eq!(
            ensure_in_yaml_config(&path, "/new/codemux-remote", OURS).unwrap(),
            EntryOutcome::Written
        );
        let parsed: serde_yaml::Value =
            serde_yaml::from_str(&read_to_string(&path)).unwrap();
        let servers = parsed["servers"].as_sequence().unwrap();
        let codemux = servers
            .iter()
            .find(|s| s["name"] == serde_yaml::Value::from("codemux"))
            .unwrap();
        assert_eq!(codemux["command"], serde_yaml::Value::from("/new/codemux-remote"));
    }

    #[test]
    fn yaml_refuses_to_overwrite_malformed_file() {
        let dir = TempDir::new().unwrap();
        let path = yaml_file(&dir, "servers:\n  - name: \"oops\n"); // unclosed quote
        let err = ensure_in_yaml_config(&path, "/bin/codemux-remote", OURS).unwrap_err();
        assert!(err.contains("parse YAML"));
    }

    #[test]
    fn yaml_does_not_create_a_file_where_the_app_owns_the_name() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("mcp-servers.yaml");

        assert_eq!(
            ensure_in_yaml_config(&path, "/bin/codemux-remote", APP).unwrap(),
            EntryOutcome::Absent
        );
        assert!(!path.exists());
    }

    #[test]
    fn yaml_leaves_a_codemux_entry_it_does_not_own() {
        let dir = TempDir::new().unwrap();
        let content =
            "servers:\n  - name: codemux\n    command: /usr/bin/codemux\n    args: [\"mcp\"]\n";
        let path = yaml_file(&dir, content);

        for owner in [OURS, APP] {
            assert_eq!(
                ensure_in_yaml_config(&path, "/bin/codemux-remote", owner).unwrap(),
                EntryOutcome::Foreign("/usr/bin/codemux".into())
            );
            assert_eq!(read_to_string(&path), content, "bytes unchanged");
        }
    }

    #[test]
    fn yaml_withdraws_its_entry_and_keeps_the_others() {
        let dir = TempDir::new().unwrap();
        let path = yaml_file(
            &dir,
            r#"servers:
  - name: shadcn
    command: npx
    args: ["@shadcn/mcp"]
  - name: codemux
    command: /home/u/.local/bin/codemux-remote
    args: ["mcp"]
  - name: github
    command: gh-mcp
"#,
        );

        assert_eq!(
            ensure_in_yaml_config(&path, "/home/u/.local/bin/codemux-remote", APP).unwrap(),
            EntryOutcome::Withdrawn
        );
        assert_eq!(yaml_names(&path), ["shadcn", "github"]);
    }

    #[test]
    fn yaml_refuses_a_non_sequence_servers_key() {
        let dir = TempDir::new().unwrap();
        let content = "servers: {}\n";
        let path = yaml_file(&dir, content);

        let err = ensure_in_yaml_config(&path, "/bin/codemux-remote", OURS).unwrap_err();
        assert!(err.contains("servers is not a list"), "{err}");
        assert_eq!(read_to_string(&path), content);
    }

    // ─── Ownership ─────────────────────────────────────────────

    #[test]
    fn name_owner_is_the_app_once_its_layout_exists() {
        for app_dir in ["codemux", "codemux-dev"] {
            let config = TempDir::new().unwrap();
            std::fs::create_dir_all(config.path().join(app_dir)).unwrap();
            std::fs::write(config.path().join(app_dir).join("layout.json"), "{}").unwrap();
            assert_eq!(name_owner(Some(config.path())), APP, "{app_dir}/layout.json");
        }

        // The host scheduler creates the database there too, so it proves nothing.
        let scheduler_only = TempDir::new().unwrap();
        std::fs::create_dir_all(scheduler_only.path().join("codemux")).unwrap();
        std::fs::write(scheduler_only.path().join("codemux").join("codemux.db"), "").unwrap();
        assert_eq!(name_owner(Some(scheduler_only.path())), OURS);

        let empty = TempDir::new().unwrap();
        assert_eq!(name_owner(Some(empty.path())), OURS);
        assert_eq!(name_owner(None), OURS);
    }

    #[test]
    fn is_codemux_remote_command_matches_by_file_name() {
        assert!(is_codemux_remote_command(Some("codemux-remote")));
        assert!(is_codemux_remote_command(Some("/x/codemux-remote")));
        assert!(!is_codemux_remote_command(Some("codemux")));
        assert!(!is_codemux_remote_command(Some("/usr/bin/codemux")));
        assert!(!is_codemux_remote_command(Some("codemux-remote.sh")));
        assert!(!is_codemux_remote_command(Some("")));
        assert!(!is_codemux_remote_command(None));
    }

    // ─── Integration via ensure_codemux_in_agent_configs ─────────

    #[test]
    fn ensure_skips_missing_configs() {
        // No claude.json, no .vexis dir → nothing to do.
        let dir = TempDir::new().unwrap();
        let report = ensure_codemux_in_agent_configs_with_home(
            Path::new("/bin/codemux-remote"),
            dir.path(),
            OURS,
        );
        assert!(report.modified.is_empty(), "no modifications when no configs present");
        assert!(report.failed.is_empty());
    }

    #[test]
    fn ensure_picks_up_both_when_both_exist() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        std::fs::write(home.join(".claude.json"), b"{}").unwrap();
        std::fs::create_dir_all(home.join(".vexis")).unwrap();
        // mcp-servers.yaml absent — should be created.

        let report =
            ensure_codemux_in_agent_configs_with_home(Path::new("/bin/codemux-remote"), home, OURS);
        assert_eq!(report.modified.len(), 2, "both configs touched: {:?}", report);
        assert!(report.failed.is_empty());
    }

    #[test]
    fn ensure_on_a_machine_with_the_app_withdraws_both_entries() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let exe = "/home/u/.local/bin/codemux-remote";
        std::fs::write(
            home.join(".claude.json"),
            format!(r#"{{"mcpServers":{{"codemux":{{"command":"{exe}","args":["mcp"]}}}}}}"#),
        )
        .unwrap();
        std::fs::create_dir_all(home.join(".vexis")).unwrap();
        std::fs::write(
            home.join(".vexis").join("mcp-servers.yaml"),
            format!("servers:\n  - name: codemux\n    command: {exe}\n    args: [\"mcp\"]\n"),
        )
        .unwrap();

        let report = ensure_codemux_in_agent_configs_with_home(Path::new(exe), home, APP);
        assert_eq!(report.removed.len(), 2, "both entries withdrawn: {report:?}");
        assert!(report.modified.is_empty() && report.failed.is_empty(), "{report:?}");
    }

    #[test]
    fn ensure_reports_foreign_entries_without_touching_them() {
        let dir = TempDir::new().unwrap();
        let home = dir.path();
        let content = r#"{"mcpServers":{"codemux":{"command":"/usr/bin/codemux","args":["mcp"]}}}"#;
        std::fs::write(home.join(".claude.json"), content).unwrap();

        let report =
            ensure_codemux_in_agent_configs_with_home(Path::new("/bin/codemux-remote"), home, OURS);
        assert_eq!(
            report.foreign,
            vec![(home.join(".claude.json"), "/usr/bin/codemux".to_string())]
        );
        assert_eq!(read_to_string(&home.join(".claude.json")), content);
    }

    // ─── yaml_quote_if_needed ──────────────────────────────────

    #[test]
    fn yaml_quote_passes_simple_paths_through() {
        assert_eq!(yaml_quote_if_needed("/home/user/.local/bin/codemux-remote"), "/home/user/.local/bin/codemux-remote");
    }

    #[test]
    fn yaml_quote_wraps_paths_with_spaces() {
        assert_eq!(
            yaml_quote_if_needed("/home/cool user/bin/codemux"),
            "\"/home/cool user/bin/codemux\""
        );
    }
}
