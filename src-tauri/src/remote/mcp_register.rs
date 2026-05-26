//! Auto-register `codemux-remote mcp` in known agent MCP configs on
//! the host, so any agent CLI the user runs on this host already
//! knows about Codemux without the user editing config files by
//! hand.
//!
//! Why this exists: the desktop push flow drops a per-workspace
//! `.mcp.json` (handled by `ssh::bootstrap::provision_workspace_mcp_config`).
//! But users also work on directories on the host that weren't
//! pushed — repos cloned directly on the server, ad-hoc scratch
//! dirs, services running in arbitrary places. For those, agents
//! need a user-level (not per-workspace) MCP config that names
//! `codemux-remote` as a server. We write that here, on every
//! `codemux-remote serve` startup, idempotently.
//!
//! Safety contract:
//!
//! 1. **Idempotent.** If the codemux entry is already present, we
//!    make no changes. Re-running on every daemon start is fine.
//! 2. **Atomic.** Writes go through a sibling `.tmp` file + rename
//!    so a crash mid-write can never leave the user's agent config
//!    in a half-baked state.
//! 3. **No corruption.** If the existing file is unparseable
//!    (broken JSON/YAML), we log and bail — never overwrite a file
//!    we can't safely round-trip.
//! 4. **Skip missing tools.** A user who doesn't have Claude Code
//!    won't have `~/.claude.json`. We skip configs whose parent
//!    directory doesn't exist. We don't create agent-specific
//!    directories the user never opted into.
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

use std::path::{Path, PathBuf};

use serde_json::{json, Value as JsonValue};

/// Outcome of scanning + updating known agent configs.
#[derive(Debug, Default)]
pub struct RegisterReport {
    /// Paths whose `codemux` entry we added or updated.
    pub modified: Vec<PathBuf>,
    /// Paths we examined that already had a current entry — no
    /// changes were made.
    pub unchanged: Vec<PathBuf>,
    /// Paths we wanted to touch but couldn't (malformed file, IO
    /// error). Each entry includes the reason for diagnostics.
    pub failed: Vec<(PathBuf, String)>,
}

/// Probe every known agent-config location and ensure each has a
/// `codemux` MCP server entry pointing at `codemux_remote_path`
/// (typically `std::env::current_exe()`).
///
/// Best-effort: returns a [`RegisterReport`] with per-path outcomes,
/// never an `Err`. Caller logs whatever it likes.
pub fn ensure_codemux_in_agent_configs(codemux_remote_path: &Path) -> RegisterReport {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return RegisterReport::default(),
    };
    ensure_codemux_in_agent_configs_with_home(codemux_remote_path, &home)
}

/// Test-friendly variant of [`ensure_codemux_in_agent_configs`] that
/// takes an explicit `home` directory. Production callers use the
/// `dirs::home_dir()`-rooted wrapper above; the `with_home` form lets
/// tests target a tempdir without mutating the process's `HOME`
/// env var (which would race other tests).
pub fn ensure_codemux_in_agent_configs_with_home(
    codemux_remote_path: &Path,
    home: &Path,
) -> RegisterReport {
    let mut report = RegisterReport::default();
    let exec = codemux_remote_path.to_string_lossy().into_owned();

    // Claude Code: ~/.claude.json
    let claude = home.join(".claude.json");
    if claude.exists() {
        match ensure_in_json_config(&claude, &exec) {
            Ok(true) => report.modified.push(claude),
            Ok(false) => report.unchanged.push(claude),
            Err(e) => report.failed.push((claude, e)),
        }
    }

    // Vexis: ~/.vexis/mcp-servers.yaml
    let vexis_dir = home.join(".vexis");
    if vexis_dir.is_dir() {
        let vexis = vexis_dir.join("mcp-servers.yaml");
        match ensure_in_yaml_config(&vexis, &exec) {
            Ok(true) => report.modified.push(vexis),
            Ok(false) => report.unchanged.push(vexis),
            Err(e) => report.failed.push((vexis, e)),
        }
    }

    report
}

/// JSON-shaped config writer. Used for `~/.claude.json` and any
/// `.mcp.json`-style file. Returns `Ok(true)` if a change was
/// written, `Ok(false)` if nothing needed updating.
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
/// stay untouched. If `mcpServers` isn't present we create it; if
/// `codemux` is already there pointing at the right command, we
/// leave the file alone.
pub fn ensure_in_json_config(path: &Path, codemux_remote_path: &str) -> Result<bool, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read: {e}"))?;
    let mut config: JsonValue = serde_json::from_slice(&bytes)
        .map_err(|e| format!("parse JSON: {e}"))?;

    // If the file isn't an object at the top level we bail rather
    // than overwrite — could be a deliberately exotic config shape.
    if !config.is_object() {
        return Err("top-level value is not a JSON object".into());
    }

    if !config.get("mcpServers").is_some_and(JsonValue::is_object) {
        config["mcpServers"] = json!({});
    }

    let desired = json!({
        "command": codemux_remote_path,
        "args": ["mcp"],
    });

    if config["mcpServers"]["codemux"] == desired {
        return Ok(false);
    }

    config["mcpServers"]["codemux"] = desired;

    let serialised = serde_json::to_string_pretty(&config)
        .map_err(|e| format!("serialise JSON: {e}"))?;
    atomic_write(path, serialised.as_bytes())?;
    Ok(true)
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
/// A vector of server entries keyed by `name`. We upsert the entry
/// named `codemux` while preserving every other entry verbatim.
pub fn ensure_in_yaml_config(path: &Path, codemux_remote_path: &str) -> Result<bool, String> {
    use serde_yaml::Value as YamlValue;

    // If the file doesn't exist yet, create a minimal one. Vexis's
    // expected schema starts with the `servers:` list at the top.
    // We do this here (vs. the JSON path's "must exist" rule)
    // because the directory existing means the user definitely
    // uses Vexis, even if they never wrote any MCP servers in
    // before.
    if !path.exists() {
        let new_doc = format!(
            "servers:\n  - name: codemux\n    command: {cmd}\n    args: [\"mcp\"]\n",
            cmd = yaml_quote_if_needed(codemux_remote_path),
        );
        atomic_write(path, new_doc.as_bytes())?;
        return Ok(true);
    }

    let bytes = std::fs::read(path).map_err(|e| format!("read: {e}"))?;
    let mut doc: YamlValue = serde_yaml::from_slice(&bytes)
        .map_err(|e| format!("parse YAML: {e}"))?;

    // Empty file → top-level becomes Null. Treat as "no servers
    // yet" and seed the structure.
    if doc.is_null() {
        doc = serde_yaml::Value::Mapping(serde_yaml::Mapping::new());
    }

    let map = doc
        .as_mapping_mut()
        .ok_or_else(|| "top-level YAML is not a mapping".to_string())?;
    let servers_key = serde_yaml::Value::String("servers".into());

    // Ensure `servers:` exists and is a sequence.
    if !map
        .get(&servers_key)
        .is_some_and(serde_yaml::Value::is_sequence)
    {
        map.insert(servers_key.clone(), serde_yaml::Value::Sequence(vec![]));
    }
    let servers = map
        .get_mut(&servers_key)
        .and_then(|v| v.as_sequence_mut())
        .expect("servers sequence we just ensured exists");

    // Build the desired entry.
    let mut desired = serde_yaml::Mapping::new();
    desired.insert("name".into(), "codemux".into());
    desired.insert("command".into(), codemux_remote_path.into());
    desired.insert(
        "args".into(),
        serde_yaml::Value::Sequence(vec!["mcp".into()]),
    );
    let desired_value = serde_yaml::Value::Mapping(desired);

    // Upsert by name.
    let existing_index = servers.iter().position(|entry| {
        entry
            .as_mapping()
            .and_then(|m| m.get(&serde_yaml::Value::String("name".into())))
            .and_then(|n| n.as_str())
            == Some("codemux")
    });

    match existing_index {
        Some(idx) if servers[idx] == desired_value => return Ok(false),
        Some(idx) => servers[idx] = desired_value,
        None => servers.push(desired_value),
    }

    let serialised = serde_yaml::to_string(&doc).map_err(|e| format!("serialise YAML: {e}"))?;
    atomic_write(path, serialised.as_bytes())?;
    Ok(true)
}

/// Atomic-write: dump to `<path>.tmp`, fsync, rename into place.
/// Same pattern the manifest writer uses. Preserves the existing
/// file's mode if we can; otherwise leaves the default umask.
fn atomic_write(path: &Path, content: &[u8]) -> Result<(), String> {
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

    {
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
    }
    std::fs::rename(&tmp, path).map_err(|e| format!("rename into place: {e}"))?;
    Ok(())
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

    fn read_to_string(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    // ─── JSON ──────────────────────────────────────────────────

    #[test]
    fn json_adds_codemux_to_existing_empty_config() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("claude.json");
        std::fs::write(&path, b"{}").unwrap();

        assert!(ensure_in_json_config(&path, "/bin/codemux-remote").unwrap());

        let parsed: JsonValue = serde_json::from_str(&read_to_string(&path)).unwrap();
        assert_eq!(parsed["mcpServers"]["codemux"]["command"], json!("/bin/codemux-remote"));
        assert_eq!(parsed["mcpServers"]["codemux"]["args"], json!(["mcp"]));
    }

    #[test]
    fn json_preserves_unrelated_servers() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("claude.json");
        std::fs::write(
            &path,
            r#"{
                "mcpServers": {
                    "shadcn": { "command": "npx", "args": ["shadcn"] },
                    "github": { "command": "gh-mcp" }
                },
                "otherTopLevel": "stays"
            }"#,
        )
        .unwrap();

        assert!(ensure_in_json_config(&path, "/bin/codemux-remote").unwrap());

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
        let path = dir.path().join("claude.json");
        std::fs::write(
            &path,
            r#"{"mcpServers":{"codemux":{"command":"/bin/codemux-remote","args":["mcp"]}}}"#,
        )
        .unwrap();
        let before = read_to_string(&path);

        // Returns Ok(false) — no change made.
        assert!(!ensure_in_json_config(&path, "/bin/codemux-remote").unwrap());
        assert_eq!(read_to_string(&path), before, "no rewrite when entry is current");
    }

    #[test]
    fn json_updates_when_command_path_changes() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("claude.json");
        std::fs::write(
            &path,
            r#"{"mcpServers":{"codemux":{"command":"/old/path","args":["mcp"]}}}"#,
        )
        .unwrap();
        assert!(ensure_in_json_config(&path, "/new/path").unwrap());
        let parsed: JsonValue = serde_json::from_str(&read_to_string(&path)).unwrap();
        assert_eq!(parsed["mcpServers"]["codemux"]["command"], json!("/new/path"));
    }

    #[test]
    fn json_refuses_to_overwrite_malformed_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("claude.json");
        std::fs::write(&path, b"this is not json {{{").unwrap();
        let err = ensure_in_json_config(&path, "/bin/codemux-remote").unwrap_err();
        assert!(err.contains("parse JSON"));
        // File untouched.
        assert_eq!(std::fs::read(&path).unwrap(), b"this is not json {{{");
    }

    #[test]
    fn json_refuses_top_level_array() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("claude.json");
        std::fs::write(&path, b"[]").unwrap();
        let err = ensure_in_json_config(&path, "/bin/codemux-remote").unwrap_err();
        assert!(err.contains("not a JSON object"));
    }

    #[test]
    fn json_preserves_file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("claude.json");
        std::fs::write(&path, b"{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        assert!(ensure_in_json_config(&path, "/bin/codemux-remote").unwrap());
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "must not widen perms on agent config");
    }

    // ─── YAML ──────────────────────────────────────────────────

    #[test]
    fn yaml_creates_missing_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("mcp-servers.yaml");
        assert!(!path.exists());

        assert!(ensure_in_yaml_config(&path, "/bin/codemux-remote").unwrap());
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
        let path = dir.path().join("mcp-servers.yaml");
        std::fs::write(
            &path,
            r#"servers:
  - name: shadcn
    command: npx
    args: ["@shadcn/mcp"]
  - name: github
    command: gh-mcp
"#,
        )
        .unwrap();
        assert!(ensure_in_yaml_config(&path, "/bin/codemux-remote").unwrap());

        let parsed: serde_yaml::Value =
            serde_yaml::from_str(&read_to_string(&path)).unwrap();
        let servers = parsed["servers"].as_sequence().unwrap();
        // Three entries: shadcn, github, codemux.
        assert_eq!(servers.len(), 3);
        let names: Vec<_> = servers
            .iter()
            .map(|s| s["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"shadcn"));
        assert!(names.contains(&"github"));
        assert!(names.contains(&"codemux"));
    }

    #[test]
    fn yaml_is_idempotent_when_already_correct() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("mcp-servers.yaml");
        std::fs::write(
            &path,
            "servers:\n  - name: codemux\n    command: /bin/codemux-remote\n    args:\n      - mcp\n",
        )
        .unwrap();
        let before = read_to_string(&path);

        assert!(!ensure_in_yaml_config(&path, "/bin/codemux-remote").unwrap());
        assert_eq!(read_to_string(&path), before, "no rewrite when entry is current");
    }

    #[test]
    fn yaml_updates_when_command_changes() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("mcp-servers.yaml");
        std::fs::write(
            &path,
            "servers:\n  - name: codemux\n    command: /old/codemux\n    args:\n      - mcp\n",
        )
        .unwrap();
        assert!(ensure_in_yaml_config(&path, "/new/codemux").unwrap());
        let parsed: serde_yaml::Value =
            serde_yaml::from_str(&read_to_string(&path)).unwrap();
        let servers = parsed["servers"].as_sequence().unwrap();
        let codemux = servers
            .iter()
            .find(|s| s["name"] == serde_yaml::Value::from("codemux"))
            .unwrap();
        assert_eq!(codemux["command"], serde_yaml::Value::from("/new/codemux"));
    }

    #[test]
    fn yaml_refuses_to_overwrite_malformed_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("mcp-servers.yaml");
        std::fs::write(&path, b"servers:\n  - name: \"oops\n").unwrap(); // unclosed quote
        let err = ensure_in_yaml_config(&path, "/bin/codemux-remote").unwrap_err();
        assert!(err.contains("parse YAML"));
    }

    // ─── Integration via ensure_codemux_in_agent_configs ─────────

    #[test]
    fn ensure_skips_missing_configs() {
        // No claude.json, no .vexis dir → nothing to do.
        let dir = TempDir::new().unwrap();
        let report =
            ensure_codemux_in_agent_configs_with_home(Path::new("/bin/codemux-remote"), dir.path());
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
            ensure_codemux_in_agent_configs_with_home(Path::new("/bin/codemux-remote"), home);
        assert_eq!(report.modified.len(), 2, "both configs touched: {:?}", report);
        assert!(report.failed.is_empty());
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
