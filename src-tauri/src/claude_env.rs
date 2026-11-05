//! The environment for the `claude --print` processes Codemux runs itself
//! (commit messages, conflict resolution, automations, utility text). Chat
//! turns go through the sidecar, which applies the same rule in
//! `sidecar/claude-agent/src/claude-env.ts`.
//!
//! Claude Code labels its API requests with `CLAUDE_CODE_ENTRYPOINT`.
//! `claude --print` labels them `sdk-cli` when the variable is unset. t3code
//! measured `sdk-*` requests draining a subscription's 5-hour window about
//! 1.8x faster than other labels (pingdotgg/t3code#25597). Codemux is the
//! entrypoint, so it says so. The variables a launching Claude Code session exports are
//! dropped so a child never joins that session.

use std::path::{Path, PathBuf};

pub const CODEMUX_CLAUDE_ENTRYPOINT: &str = "codemux";

/// Exported by the Claude Code session that launched Codemux. A child
/// started with them acts as part of that session.
pub const CLAUDE_HOST_SESSION_ENV_VARS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_HOST_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_DESKTOP_APP_VERSION",
    "CLAUDE_EFFORT",
    "CLAUDE_PID",
    // Unlike the sidecar's list: a `--print` child is not an SDK client, so
    // an inherited SDK version would misreport it.
    "CLAUDE_AGENT_SDK_VERSION",
];

/// Claude Code's managed-settings file (its hardcoded per-OS location).
fn managed_settings_path() -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/ClaudeCode/managed-settings.json")
    } else if cfg!(windows) {
        PathBuf::from(r"C:\Program Files\ClaudeCode\managed-settings.json")
    } else {
        PathBuf::from("/etc/claude-code/managed-settings.json")
    }
}

/// User settings first, managed settings last: later files win. A relative
/// `CLAUDE_CONFIG_DIR` is resolved the way the Claude child resolves it,
/// against the folder it runs in.
fn settings_paths(config_dir: Option<PathBuf>, cwd: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let config_dir = config_dir
        .map(|dir| cwd.join(dir))
        .or_else(|| dirs::home_dir().map(|home| home.join(".claude")));
    if let Some(dir) = config_dir {
        paths.push(dir.join("settings.json"));
    }
    paths.push(managed_settings_path());
    paths
}

fn non_empty_env(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// `env.CLAUDE_CODE_ENTRYPOINT` from one settings file.
fn entrypoint_in(path: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(path).ok()?;
    let settings: serde_json::Value = serde_json::from_str(&contents).ok()?;
    let value = settings.get("env")?.get("CLAUDE_CODE_ENTRYPOINT")?.as_str()?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn entrypoint_from(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .filter_map(|path| entrypoint_in(path))
        .last()
        .unwrap_or_else(|| CODEMUX_CLAUDE_ENTRYPOINT.to_string())
}

/// The label for a Claude process Codemux starts in `cwd`. A value in
/// Claude's settings `env` wins: Claude Code reads the entrypoint only from
/// its process env and ignores that setting, so Codemux applies it.
pub fn claude_entrypoint(cwd: &Path) -> String {
    entrypoint_from(&settings_paths(non_empty_env("CLAUDE_CONFIG_DIR"), cwd))
}

/// Label a `claude` child that runs in `cwd` as Codemux and detach it from
/// a launching Claude Code session.
pub fn apply_claude_cli_env(command: &mut tokio::process::Command, cwd: &Path) {
    for name in CLAUDE_HOST_SESSION_ENV_VARS {
        command.env_remove(name);
    }
    command.env("CLAUDE_CODE_ENTRYPOINT", claude_entrypoint(cwd));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_codemux_without_settings() {
        let dir = tempfile::tempdir().unwrap();
        let paths = vec![dir.path().join("settings.json")];
        assert_eq!(entrypoint_from(&paths), "codemux");
    }

    #[test]
    fn settings_env_overrides_and_managed_settings_win() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("settings.json");
        let managed = dir.path().join("managed-settings.json");
        std::fs::write(&user, r#"{"env":{"CLAUDE_CODE_ENTRYPOINT":"mine"}}"#).unwrap();
        assert_eq!(entrypoint_from(&[user.clone(), managed.clone()]), "mine");
        std::fs::write(&managed, r#"{"env":{"CLAUDE_CODE_ENTRYPOINT":"org"}}"#).unwrap();
        assert_eq!(entrypoint_from(&[user, managed]), "org");
    }

    #[test]
    fn ignores_blank_and_malformed_settings() {
        let dir = tempfile::tempdir().unwrap();
        let blank = dir.path().join("blank.json");
        let broken = dir.path().join("broken.json");
        std::fs::write(&blank, r#"{"env":{"CLAUDE_CODE_ENTRYPOINT":"  "}}"#).unwrap();
        std::fs::write(&broken, "{not json").unwrap();
        assert_eq!(entrypoint_from(&[blank, broken]), "codemux");
    }

    #[test]
    fn config_dir_selects_the_user_settings_file() {
        let root = tempfile::tempdir().unwrap();
        let absolute = root.path().join("cfg");
        let paths = settings_paths(Some(absolute.clone()), Path::new("/elsewhere"));
        assert_eq!(paths[0], absolute.join("settings.json"));
    }

    #[test]
    fn relative_config_dir_resolves_against_the_child_folder() {
        let cwd = tempfile::tempdir().unwrap();
        std::fs::create_dir(cwd.path().join("cfg")).unwrap();
        std::fs::write(
            cwd.path().join("cfg/settings.json"),
            r#"{"env":{"CLAUDE_CODE_ENTRYPOINT":"relative"}}"#,
        )
        .unwrap();
        let paths = settings_paths(Some(PathBuf::from("cfg")), cwd.path());
        assert_eq!(paths[0], cwd.path().join("cfg/settings.json"));
        assert_eq!(entrypoint_from(&paths), "relative");
    }

    #[test]
    fn managed_settings_come_last_at_claude_codes_location() {
        let paths = settings_paths(None, Path::new("."));
        let managed = paths.last().unwrap().to_string_lossy().to_string();
        #[cfg(windows)]
        assert_eq!(managed, r"C:\Program Files\ClaudeCode\managed-settings.json");
        #[cfg(target_os = "macos")]
        assert_eq!(managed, "/Library/Application Support/ClaudeCode/managed-settings.json");
        #[cfg(all(unix, not(target_os = "macos")))]
        assert_eq!(managed, "/etc/claude-code/managed-settings.json");
    }

    #[test]
    fn claude_cli_env_labels_codemux_and_drops_host_session() {
        let mut command = tokio::process::Command::new("claude");
        apply_claude_cli_env(&mut command, Path::new("."));
        let envs: Vec<_> = command.as_std().get_envs().collect();
        assert!(envs.iter().any(|(k, v)| *k == "CLAUDE_CODE_ENTRYPOINT" && v.is_some()));
        for name in CLAUDE_HOST_SESSION_ENV_VARS {
            assert!(envs.iter().any(|(k, v)| k == name && v.is_none()), "{name} kept");
        }
    }
}
