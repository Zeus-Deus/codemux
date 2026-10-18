//! Where each account's CLI home lives, what it shares with the CLI's own
//! home, and carrying a chat's native transcript between homes.
//!
//! An extra account's home holds its own sign-in and its own transcripts, so
//! history (and therefore usage) stays attributable per account. User
//! configuration is symlinked from the CLI's own home so instructions,
//! skills, agents, and settings behave the same on every account. The list
//! is an allow-list: anything unknown (state databases, caches, credentials)
//! stays private to the account.

use std::io;
use std::path::{Path, PathBuf};

use crate::agent_provider::ProviderKind;

/// Shared from `~/.claude` (or `$CLAUDE_CONFIG_DIR`). `.claude.json` and
/// `.credentials.json` hold the account itself and are never shared; the MCP
/// servers kept in `.claude.json` are copied over instead (see
/// [`share_claude_mcp`]).
const CLAUDE_SHARED: &[&str] = &[
    "settings.json",
    "CLAUDE.md",
    "agents",
    "commands",
    "skills",
    "plugins",
    "hooks",
    "output-styles",
    "keybindings.json",
];

/// Shared from `~/.codex` (or `$CODEX_HOME`). `auth.json` is the account.
const CODEX_SHARED: &[&str] = &["config.toml", "AGENTS.md", "prompts", "skills", "plugins", "rules"];

/// The environment variable that points `provider`'s CLI at a home.
pub fn home_env_key(provider: ProviderKind) -> Option<&'static str> {
    match provider {
        ProviderKind::Claude => Some("CLAUDE_CONFIG_DIR"),
        ProviderKind::Codex => Some("CODEX_HOME"),
        _ => None,
    }
}

/// The CLI's own home: what it uses when Codemux sets nothing.
pub fn default_home(provider: ProviderKind) -> Option<PathBuf> {
    let key = home_env_key(provider)?;
    if let Some(dir) = std::env::var_os(key).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let home = dirs::home_dir()?;
    Some(match provider {
        ProviderKind::Claude => home.join(".claude"),
        _ => home.join(".codex"),
    })
}

/// Parent of every extra account's home.
pub fn accounts_root() -> Option<PathBuf> {
    Some(
        dirs::config_dir()?
            .join(crate::APP_DIR_NAME)
            .join("provider-accounts"),
    )
}

/// `<root>/<provider>/<id>`.
pub fn account_home(root: &Path, provider: &str, id: &str) -> PathBuf {
    root.join(provider).join(id)
}

/// Create `home` and link the shared configuration into it. Idempotent and
/// non-destructive: an entry the account already has (its own file, or a
/// link made earlier) is left alone.
pub fn prepare_home(provider: ProviderKind, home: &Path) -> io::Result<()> {
    std::fs::create_dir_all(home)?;
    let Some(source) = default_home(provider) else {
        return Ok(());
    };
    link_shared(provider, &source, home)?;
    if provider == ProviderKind::Claude && !same_dir(&source, home) {
        if let Some(state) = super::probe::claude_state_file(&source, true) {
            if let Err(error) = share_claude_mcp(&state, &home.join(".claude.json")) {
                eprintln!("[codemux::provider_accounts] could not share MCP servers with {}: {error}", home.display());
            }
        }
    }
    Ok(())
}

/// The MCP settings Claude keeps in `.claude.json`: user-scope servers at the
/// top, and per project its local servers and `.mcp.json` approvals.
const CLAUDE_MCP_KEYS: &[&str] = &["mcpServers"];
const CLAUDE_PROJECT_MCP_KEYS: &[&str] =
    &["mcpServers", "enabledMcpjsonServers", "disabledMcpjsonServers", "enableAllProjectMcpServers"];

/// Serializes Codemux's own writes of account state files.
static CLAUDE_STATE_WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Mirror the CLI's own MCP servers into an account's `.claude.json`, which
/// otherwise holds only that account's sign-in and state. The CLI's own
/// configuration wins, so servers are managed in one place. Written only when
/// something changed.
///
/// Claude rewrites this file itself without a lock, so the update is
/// optimistic: the file is read again just before the replacement lands, and
/// if Claude saved in between, the merge starts over from what it saved.
fn share_claude_mcp(source: &Path, target: &Path) -> io::Result<()> {
    use serde_json::Value;
    let Some(source) = read_state(source)?.and_then(|raw| parse_state(&raw)) else {
        return Ok(());
    };
    // Write through a link to the file it names.
    let target = std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
    let _guard = CLAUDE_STATE_WRITE.lock().unwrap_or_else(|e| e.into_inner());
    for _ in 0..3 {
        let raw = read_state(&target)?;
        // An unreadable or half-written file is left for the next start.
        let Some(current) = raw.as_deref().map_or(Some(Default::default()), parse_state) else {
            return Ok(());
        };
        let Some(next) = with_mcp_from(&source, current) else {
            return Ok(());
        };
        let partial = target.with_file_name(format!(".claude.json.codemux-{}", std::process::id()));
        let _ = std::fs::remove_file(&partial);
        {
            use std::io::Write;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            // MCP entries often carry tokens in `env`: the copy starts
            // private and then takes the mode of the file it replaces.
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            let mut file = options.open(&partial)?;
            file.write_all(&serde_json::to_vec_pretty(&Value::Object(next))?)?;
        }
        if let Ok(meta) = std::fs::metadata(&target) {
            std::fs::set_permissions(&partial, meta.permissions())?;
        }
        if read_state(&target)? != raw {
            let _ = std::fs::remove_file(&partial);
            continue;
        }
        return std::fs::rename(&partial, &target);
    }
    Ok(())
}

/// The file's text, or `None` when there is none yet.
fn read_state(path: &Path) -> io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(raw) => Ok(Some(raw)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn parse_state(raw: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    match serde_json::from_str(raw).ok()? {
        serde_json::Value::Object(map) => Some(map),
        _ => None,
    }
}

/// `target` with the MCP settings of `source`, or `None` when they already
/// match.
fn with_mcp_from(
    source: &serde_json::Map<String, serde_json::Value>,
    mut target: serde_json::Map<String, serde_json::Value>,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    use serde_json::{Map, Value};
    let before = target.clone();
    mirror(source, &mut target, CLAUDE_MCP_KEYS);

    let empty = Map::new();
    let source_projects = source.get("projects").and_then(Value::as_object).unwrap_or(&empty);
    let mut projects = match target.remove("projects") {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };
    for (path, settings) in source_projects {
        let Some(settings) = settings.as_object() else { continue };
        if !CLAUDE_PROJECT_MCP_KEYS.iter().any(|k| settings.contains_key(*k)) && !projects.contains_key(path) {
            continue;
        }
        let entry = projects.entry(path.clone()).or_insert_with(|| Value::Object(Map::new()));
        if let Some(entry) = entry.as_object_mut() {
            mirror(settings, entry, CLAUDE_PROJECT_MCP_KEYS);
        }
    }
    for (path, entry) in projects.iter_mut() {
        if source_projects.contains_key(path) {
            continue;
        }
        if let Some(entry) = entry.as_object_mut() {
            mirror(&empty, entry, CLAUDE_PROJECT_MCP_KEYS);
        }
    }
    if !projects.is_empty() || before.contains_key("projects") {
        target.insert("projects".into(), Value::Object(projects));
    }
    (target != before).then_some(target)
}

fn mirror(from: &serde_json::Map<String, serde_json::Value>, to: &mut serde_json::Map<String, serde_json::Value>, keys: &[&str]) {
    for key in keys {
        match from.get(*key) {
            Some(value) => {
                to.insert((*key).to_string(), value.clone());
            }
            None => {
                to.remove(*key);
            }
        }
    }
}

fn shared_entries(provider: ProviderKind) -> &'static [&'static str] {
    match provider {
        ProviderKind::Claude => CLAUDE_SHARED,
        ProviderKind::Codex => CODEX_SHARED,
        _ => &[],
    }
}

fn link_shared(provider: ProviderKind, source: &Path, home: &Path) -> io::Result<()> {
    if same_dir(source, home) {
        return Ok(());
    }
    for name in shared_entries(provider) {
        let from = source.join(name);
        let to = home.join(name);
        if !from.exists() {
            continue;
        }
        if let Ok(meta) = std::fs::symlink_metadata(&to) {
            // A real file or folder is the account's own; a link that still
            // points at the shared entry is current. Repoint a link left
            // over from an older default home (say `CLAUDE_CONFIG_DIR` moved).
            if !meta.file_type().is_symlink() || std::fs::read_link(&to).is_ok_and(|t| t == from) {
                continue;
            }
            if let Err(error) = remove_link(&to) {
                eprintln!("[codemux::provider_accounts] could not repoint {}: {error}", to.display());
                continue;
            }
        }
        if let Err(error) = link(&from, &to) {
            eprintln!(
                "[codemux::provider_accounts] could not share {} with {}: {error}",
                from.display(),
                home.display()
            );
        }
    }
    Ok(())
}

#[cfg(unix)]
fn link(from: &Path, to: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(from, to)
}

/// Windows symlinks need Developer Mode; files fall back to a copy taken
/// now, folders stay unshared.
#[cfg(windows)]
fn link(from: &Path, to: &Path) -> io::Result<()> {
    if from.is_dir() {
        std::os::windows::fs::symlink_dir(from, to)
    } else {
        std::os::windows::fs::symlink_file(from, to).or_else(|_| std::fs::copy(from, to).map(|_| ()))
    }
}

/// Remove a link without following it. Windows keeps folder links as
/// directories.
fn remove_link(path: &Path) -> io::Result<()> {
    std::fs::remove_file(path).or_else(|error| if cfg!(windows) { std::fs::remove_dir(path) } else { Err(error) })
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Copy the native transcript of `native_id` from one home to the other so
/// the chat resumes with its full context on the new account. Returns
/// whether a transcript was found and copied. The copy replaces an older
/// one left by an earlier switch: the source is always the newest history.
pub fn carry_transcript(
    provider: ProviderKind,
    from_home: &Path,
    to_home: &Path,
    native_id: &str,
) -> io::Result<bool> {
    if !is_plausible_id(native_id) || same_dir(from_home, to_home) {
        return Ok(false);
    }
    match provider {
        ProviderKind::Claude => carry_claude(from_home, to_home, native_id),
        ProviderKind::Codex => carry_codex(from_home, to_home, native_id),
        _ => Ok(false),
    }
}

fn is_plausible_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 200
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `projects/<cwd slug>/<id>.jsonl`, plus the `<id>/` folder beside it that
/// holds subagent transcripts and large tool results.
fn carry_claude(from: &Path, to: &Path, id: &str) -> io::Result<bool> {
    let projects = from.join("projects");
    let Ok(entries) = std::fs::read_dir(&projects) else {
        return Ok(false);
    };
    let file = format!("{id}.jsonl");
    for entry in entries.flatten() {
        let dir = entry.path();
        let transcript = dir.join(&file);
        if !transcript.is_file() {
            continue;
        }
        let target_dir = to.join("projects").join(entry.file_name());
        std::fs::create_dir_all(&target_dir)?;
        std::fs::copy(&transcript, target_dir.join(&file))?;
        let side = dir.join(id);
        if side.is_dir() {
            copy_tree(&side, &target_dir.join(id))?;
        }
        return Ok(true);
    }
    Ok(false)
}

/// `sessions/YYYY/MM/DD/rollout-<timestamp>-<id>.jsonl`, kept at the same
/// relative path so Codex finds it where it would have written it. Codex
/// compresses idle rollouts to `.jsonl.zst` and reads either; the plain file
/// wins while both exist, as it does for Codex.
fn carry_codex(from: &Path, to: &Path, id: &str) -> io::Result<bool> {
    let sessions = from.join("sessions");
    let rollout = |suffix: String| {
        find_file(&sessions, &|name: &str| name.starts_with("rollout-") && name.ends_with(&suffix))
    };
    let Some(found) = rollout(format!("-{id}.jsonl")).or_else(|| rollout(format!("-{id}.jsonl.zst"))) else {
        return Ok(false);
    };
    let Ok(relative) = found.strip_prefix(from) else {
        return Ok(false);
    };
    let target = to.join(relative);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let name = target.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
    let partial = target.with_file_name(format!(".{name}.partial"));
    std::fs::copy(&found, &partial)?;
    std::fs::rename(&partial, &target)?;
    // A copy left by an earlier switch in the other form would shadow this
    // one (Codex reads the plain file first), so only the fresh copy stays.
    let other = match name.strip_suffix(".zst") {
        Some(plain) => target.with_file_name(plain),
        None => target.with_file_name(format!("{name}.zst")),
    };
    match std::fs::remove_file(&other) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    Ok(true)
}

fn find_file(dir: &Path, matches: &dyn Fn(&str) -> bool) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut dirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            dirs.push(path);
        } else if entry.file_name().to_str().is_some_and(matches) {
            return Some(path);
        }
    }
    // Newest day first: a resumed chat is usually recent.
    dirs.sort();
    dirs.into_iter().rev().find_map(|d| find_file(&d, matches))
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Whether Claude's transcript for `id` is definitely absent from `home`.
/// Same contract as the default-home preflight in `agent_chat`.
pub fn claude_transcript_missing(home: &Path, id: &str) -> bool {
    crate::commands::agent_chat::claude_session_file_missing_in(home, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn shared_links_follow_a_moved_default_home_but_own_files_stay() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("old");
        let new = tmp.path().join("new");
        let home = tmp.path().join("acct");
        for dir in [&old, &new, &home] {
            std::fs::create_dir_all(dir).unwrap();
        }
        for dir in [&old, &new] {
            std::fs::write(dir.join("settings.json"), "{}").unwrap();
            std::fs::write(dir.join("CLAUDE.md"), "shared").unwrap();
        }
        link_shared(ProviderKind::Claude, &old, &home).unwrap();
        std::fs::remove_file(home.join("CLAUDE.md")).unwrap();
        std::fs::write(home.join("CLAUDE.md"), "own").unwrap();

        link_shared(ProviderKind::Claude, &new, &home).unwrap();
        assert_eq!(std::fs::read_link(home.join("settings.json")).unwrap(), new.join("settings.json"));
        assert_eq!(std::fs::read_to_string(home.join("CLAUDE.md")).unwrap(), "own");
    }

    #[test]
    fn claude_mcp_servers_follow_the_cli_but_the_sign_in_stays() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("cli.json");
        let target = tmp.path().join("acct.json");
        std::fs::write(
            &source,
            r#"{"oauthAccount":{"emailAddress":"me@home.dev"},
                "mcpServers":{"db":{"command":"db-mcp"}},
                "projects":{"/repo":{"mcpServers":{"local":{"command":"x"}},"enabledMcpjsonServers":["team"],"history":[1]}}}"#,
        )
        .unwrap();
        std::fs::write(
            &target,
            r#"{"oauthAccount":{"emailAddress":"work@acme.dev"},"mcpServers":{"old":{}},
                "projects":{"/gone":{"mcpServers":{"old":{}},"allowedTools":["Bash"]}}}"#,
        )
        .unwrap();

        share_claude_mcp(&source, &target).unwrap();
        let state: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
        assert_eq!(state["oauthAccount"]["emailAddress"], "work@acme.dev");
        assert_eq!(state["mcpServers"], serde_json::json!({ "db": { "command": "db-mcp" } }));
        assert_eq!(state["projects"]["/repo"]["mcpServers"]["local"]["command"], "x");
        assert_eq!(state["projects"]["/repo"]["enabledMcpjsonServers"], serde_json::json!(["team"]));
        assert!(state["projects"]["/repo"].get("history").is_none());
        assert!(state["projects"]["/gone"].get("mcpServers").is_none());
        assert_eq!(state["projects"]["/gone"]["allowedTools"], serde_json::json!(["Bash"]));

        // Nothing changed: the file is not rewritten.
        let written = std::fs::metadata(&target).unwrap().modified().unwrap();
        share_claude_mcp(&source, &target).unwrap();
        assert_eq!(std::fs::metadata(&target).unwrap().modified().unwrap(), written);
    }

    #[cfg(unix)]
    #[test]
    fn the_account_state_file_stays_private_and_linked() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("cli.json");
        std::fs::write(&source, r#"{"mcpServers":{"db":{"env":{"TOKEN":"x"}}}}"#).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;

        let fresh = tmp.path().join("fresh.json");
        share_claude_mcp(&source, &fresh).unwrap();
        assert_eq!(mode(&fresh), 0o600);

        let real = tmp.path().join("real.json");
        std::fs::write(&real, "{}").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o640)).unwrap();
        let link = tmp.path().join("link.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        share_claude_mcp(&source, &link).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(mode(&real), 0o640);
        assert!(std::fs::read_to_string(&real).unwrap().contains("TOKEN"));
    }

    #[test]
    fn claude_transcript_and_its_side_folder_move_to_the_new_home() {
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("a");
        let to = tmp.path().join("b");
        let project = from.join("projects").join("-home-me-repo");
        std::fs::create_dir_all(project.join("sess-1").join("subagents")).unwrap();
        std::fs::write(project.join("sess-1.jsonl"), "{\"a\":1}\n").unwrap();
        std::fs::write(project.join("sess-1").join("subagents").join("x.jsonl"), "{}\n").unwrap();
        std::fs::write(project.join("other.jsonl"), "{}\n").unwrap();

        assert!(carry_transcript(ProviderKind::Claude, &from, &to, "sess-1").unwrap());
        let moved = to.join("projects").join("-home-me-repo");
        assert_eq!(std::fs::read_to_string(moved.join("sess-1.jsonl")).unwrap(), "{\"a\":1}\n");
        assert!(moved.join("sess-1").join("subagents").join("x.jsonl").is_file());
        assert!(!moved.join("other.jsonl").exists());
        assert!(!claude_transcript_missing(&to, "sess-1"));
    }

    #[test]
    fn codex_rollout_keeps_its_relative_path() {
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("a");
        let to = tmp.path().join("b");
        let day = from.join("sessions").join("2026").join("10").join("18");
        std::fs::create_dir_all(&day).unwrap();
        let name = "rollout-2026-10-18T10-00-00-0199aaaa-bbbb-cccc.jsonl";
        std::fs::write(day.join(name), "{}\n").unwrap();

        assert!(carry_transcript(ProviderKind::Codex, &from, &to, "0199aaaa-bbbb-cccc").unwrap());
        assert!(to.join("sessions/2026/10/18").join(name).is_file());
        assert!(!carry_transcript(ProviderKind::Codex, &from, &to, "missing").unwrap());
    }

    #[test]
    fn a_compressed_codex_rollout_still_carries() {
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("a");
        let to = tmp.path().join("b");
        let day = from.join("sessions").join("2026").join("09").join("01");
        std::fs::create_dir_all(&day).unwrap();
        let name = "rollout-2026-09-01T10-00-00-0199dddd-eeee.jsonl.zst";
        std::fs::write(day.join(name), [0x28, 0xb5, 0x2f, 0xfd]).unwrap();

        // An older plain copy from an earlier switch would shadow it.
        let dest = to.join("sessions/2026/09/01");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join(name.trim_end_matches(".zst")), "old\n").unwrap();

        assert!(carry_transcript(ProviderKind::Codex, &from, &to, "0199dddd-eeee").unwrap());
        assert!(dest.join(name).is_file());
        assert!(!dest.join(name.trim_end_matches(".zst")).exists());
    }

    #[test]
    fn unsafe_ids_are_never_used_as_paths() {
        let tmp = tempfile::tempdir().unwrap();
        for id in ["", "../x", "a/b", "a\\b"] {
            assert!(!carry_transcript(ProviderKind::Claude, tmp.path(), &tmp.path().join("b"), id).unwrap());
        }
    }

    #[cfg(unix)]
    #[test]
    fn shared_config_links_but_credentials_and_history_stay_private() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("default");
        let home = tmp.path().join("work");
        std::fs::create_dir_all(source.join("skills")).unwrap();
        std::fs::create_dir_all(source.join("projects")).unwrap();
        std::fs::write(source.join("settings.json"), "{}").unwrap();
        std::fs::write(source.join(".credentials.json"), "secret").unwrap();
        std::fs::create_dir_all(&home).unwrap();
        // An entry the account already owns is never replaced.
        std::fs::write(home.join("CLAUDE.md"), "mine").unwrap();
        std::fs::write(source.join("CLAUDE.md"), "shared").unwrap();

        link_shared(ProviderKind::Claude, &source, &home).unwrap();
        link_shared(ProviderKind::Claude, &source, &home).unwrap();
        assert!(std::fs::symlink_metadata(home.join("settings.json")).unwrap().file_type().is_symlink());
        assert!(home.join("skills").is_dir());
        assert_eq!(std::fs::read_to_string(home.join("CLAUDE.md")).unwrap(), "mine");
        assert!(!home.join(".credentials.json").exists());
        assert!(!home.join("projects").exists());
    }
}
