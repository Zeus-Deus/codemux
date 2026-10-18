//! App-owned idle compaction for Claude chats.
//!
//! Claude Code compacts a long conversation that the user stepped away from
//! before its prompt cache expires, so the next message re-sends a summary
//! instead of the whole history. Agent SDK sessions (what a Codemux chat is)
//! never do this on their own. Deciding *when* to compact is a plugin's job
//! (`host.chat.requestCompaction`); this module and the session own the
//! *safe execution*:
//!
//! * the request is refused unless the session is idle, ready and has
//!   nothing queued, re-checked under the session lock;
//! * the session is claimed, so a user send that arrives meanwhile queues
//!   behind the compaction;
//! * the compaction's own turn and transcript output is hidden; only its
//!   context and usage facts pass, plus an info notice when it compacted;
//! * an unsettled compaction is abandoned after [`IN_FLIGHT_TIMEOUT`];
//! * Claude Code's own opt-outs apply: `"idleCompaction": false` or
//!   `"autoCompactEnabled": false` in Claude settings, and
//!   `DISABLE_COMPACT` / `DISABLE_AUTO_COMPACT`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

/// A compaction that has not settled by now is abandoned so queued
/// follow-ups are never stuck behind it.
pub const IN_FLIGHT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Leading text of the transcript notice. The frontend matches on it to
/// render the row as a quiet informational line; the rest is the copy.
pub const NOTICE_PREFIX: &str = "idle-compaction: ";
/// The plugin decides the timing, so the copy only states what happened.
pub const NOTICE_TEXT: &str = "Compacted while idle";

pub fn notice_message() -> String {
    format!("{NOTICE_PREFIX}{NOTICE_TEXT}")
}

/// Why a compaction request was not started. `as_str` is the stable reason
/// a plugin sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// A turn, a pending approval, a queue dispatch or another compaction
    /// is in flight.
    Busy,
    /// User turns are waiting in the follow-up queue.
    Queued,
    /// The session is starting, errored, or its query cannot take a
    /// prompt as-is.
    NotReady,
    /// The session is closed.
    Closed,
    /// No live session for the thread.
    NoSession,
    /// The thread is not a Claude chat.
    NotClaude,
    /// A Claude chat that cannot be compacted from here (device chats,
    /// managed workflow attempts).
    Unsupported,
    /// `DISABLE_COMPACT`, `DISABLE_AUTO_COMPACT` or `autoCompactEnabled: false`.
    CompactionOff,
    /// `idleCompaction: false`.
    IdleCompactionOff,
    /// The sidecar did not answer the request.
    Failed,
}

impl Refusal {
    pub fn as_str(self) -> &'static str {
        match self {
            Refusal::Busy => "busy",
            Refusal::Queued => "queued",
            Refusal::NotReady => "not_ready",
            Refusal::Closed => "closed",
            Refusal::NoSession => "no_session",
            Refusal::NotClaude => "not_claude",
            Refusal::Unsupported => "unsupported",
            Refusal::CompactionOff => "compaction_off",
            Refusal::IdleCompactionOff => "idle_compaction_off",
            Refusal::Failed => "failed",
        }
    }
}

// ---------------------------------------------------------------------------
// Claude Code's own opt-outs
// ---------------------------------------------------------------------------

/// Settings layers the session's CLI reads, highest precedence first.
#[derive(Debug, Default)]
pub struct ClaudeSettings {
    layers: Vec<Value>,
    /// `~/.claude.json`, where older CLIs keep `autoCompactEnabled`.
    legacy_global: Option<Value>,
}

impl ClaudeSettings {
    /// Read the layers for a session in `cwd`. `flag` is the settings
    /// object Codemux passes to the CLI with `--settings`; `env` is the
    /// environment the CLI runs with (for `CLAUDE_CONFIG_DIR` and `HOME`).
    pub fn load(cwd: &Path, flag: Option<&Value>, env: &dyn Fn(&str) -> Option<String>) -> Self {
        let home = env("HOME")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .or_else(dirs::home_dir);
        let config_dir = env("CLAUDE_CONFIG_DIR")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|home| home.join(".claude")));
        let mut layers = Vec::new();
        layers.extend(read_json(&managed_settings_path()));
        layers.extend(flag.filter(|v| v.is_object()).cloned());
        layers.extend(read_json(&cwd.join(".claude").join("settings.local.json")));
        layers.extend(read_json(&cwd.join(".claude").join("settings.json")));
        if let Some(dir) = config_dir.as_ref() {
            layers.extend(read_json(&dir.join("settings.json")));
        }
        let legacy_global = home.and_then(|home| read_json(&home.join(".claude.json")));
        Self {
            layers,
            legacy_global,
        }
    }

    #[cfg(test)]
    fn from_layers(layers: Vec<Value>, legacy_global: Option<Value>) -> Self {
        Self {
            layers,
            legacy_global,
        }
    }

    fn setting(&self, key: &str) -> Option<&Value> {
        self.layers.iter().find_map(|layer| layer.get(key))
    }

    fn env(&self, key: &str, process: &dyn Fn(&str) -> Option<String>) -> Option<String> {
        process(key).or_else(|| {
            self.layers.iter().find_map(|layer| {
                layer
                    .get("env")
                    .and_then(|env| env.get(key))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
        })
    }

    /// The opt-out that applies, if any.
    pub fn opt_out(&self, process: &dyn Fn(&str) -> Option<String>) -> Option<Refusal> {
        let truthy = |key: &str| self.env(key, process).is_some_and(|v| is_truthy(&v));
        if truthy("DISABLE_COMPACT") || truthy("DISABLE_AUTO_COMPACT") {
            return Some(Refusal::CompactionOff);
        }
        let auto = self.setting("autoCompactEnabled").or_else(|| {
            self.legacy_global
                .as_ref()
                .and_then(|g| g.get("autoCompactEnabled"))
        });
        if auto.and_then(Value::as_bool) == Some(false) {
            return Some(Refusal::CompactionOff);
        }
        if self.setting("idleCompaction").and_then(Value::as_bool) == Some(false) {
            return Some(Refusal::IdleCompactionOff);
        }
        None
    }
}

pub fn process_env(key: &str) -> Option<String> {
    std::env::var(key).ok()
}

fn is_truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn read_json(path: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<Value>(&text)
        .ok()
        .filter(Value::is_object)
}

fn managed_settings_path() -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/ClaudeCode/managed-settings.json")
    } else if cfg!(windows) {
        PathBuf::from(r"C:\Program Files\ClaudeCode\managed-settings.json")
    } else {
        PathBuf::from("/etc/claude-code/managed-settings.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn opt_outs_match_claude_code() {
        let none = ClaudeSettings::from_layers(vec![json!({})], None);
        assert_eq!(none.opt_out(&no_env), None);

        let idle_off = ClaudeSettings::from_layers(vec![json!({"idleCompaction": false})], None);
        assert_eq!(idle_off.opt_out(&no_env), Some(Refusal::IdleCompactionOff));

        let auto_off =
            ClaudeSettings::from_layers(vec![json!({"autoCompactEnabled": false})], None);
        assert_eq!(auto_off.opt_out(&no_env), Some(Refusal::CompactionOff));

        let legacy =
            ClaudeSettings::from_layers(vec![], Some(json!({"autoCompactEnabled": false})));
        assert_eq!(legacy.opt_out(&no_env), Some(Refusal::CompactionOff));

        let env_layer =
            ClaudeSettings::from_layers(vec![json!({"env": {"DISABLE_AUTO_COMPACT": "1"}})], None);
        assert_eq!(env_layer.opt_out(&no_env), Some(Refusal::CompactionOff));

        let process = |key: &str| (key == "DISABLE_COMPACT").then(|| "true".to_string());
        assert_eq!(none.opt_out(&process), Some(Refusal::CompactionOff));
    }

    #[test]
    fn the_highest_settings_layer_wins() {
        let local_on = ClaudeSettings::from_layers(
            vec![
                json!({"idleCompaction": true}),
                json!({"idleCompaction": false}),
            ],
            None,
        );
        assert_eq!(local_on.opt_out(&no_env), None);
    }

    #[test]
    fn settings_files_are_read_from_the_project() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        std::fs::write(
            dir.path().join(".claude").join("settings.local.json"),
            r#"{"idleCompaction": false}"#,
        )
        .unwrap();
        let home = tempfile::tempdir().unwrap();
        let home_dir = home.path().to_string_lossy().to_string();
        let env = move |key: &str| (key == "HOME").then(|| home_dir.clone());
        let settings = ClaudeSettings::load(dir.path(), None, &env);
        assert_eq!(settings.opt_out(&no_env), Some(Refusal::IdleCompactionOff));
    }

    #[test]
    fn user_settings_follow_claude_config_dir() {
        let project = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        std::fs::write(
            config.path().join("settings.json"),
            r#"{"autoCompactEnabled": false}"#,
        )
        .unwrap();
        let dir = config.path().to_string_lossy().to_string();
        let env = move |key: &str| (key == "CLAUDE_CONFIG_DIR").then(|| dir.clone());
        let settings = ClaudeSettings::load(project.path(), None, &env);
        assert_eq!(settings.opt_out(&no_env), Some(Refusal::CompactionOff));
    }

    #[test]
    fn the_notice_carries_the_prefix_the_frontend_matches() {
        assert!(notice_message().starts_with(NOTICE_PREFIX));
    }
}
