//! Dynamic slash-command discovery for ACP-backed providers.
//!
//! An ACP agent publishes its catalogue as an `available_commands_update`
//! session notification, and some agents additionally answer with a full
//! catalogue in `initialize._meta.availableCommands`. Command names and
//! argument hints stay opaque so a newer CLI can add commands without a
//! Codemux release.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::Value;
use tokio::sync::Mutex;

use crate::agent_provider::claude::slash_commands::ProviderSlashCommand;

use super::session::AcpDialect;

/// This command silently broadens tool permissions inside the agent without
/// updating Codemux's permission-mode control. Exposing it would leave the
/// visible safety state out of sync with the child process.
const ACP_UNSAFE_COMMANDS: &[&str] = &["always-approve"];

/// Latest authoritative command snapshot for each dialect and workspace cwd.
///
/// A running session fills and replaces entries from
/// `available_commands_update`; for dialects that also answer at initialize,
/// a cheap probe fills a missing entry lazily.
///
/// The dialect is part of the key rather than bound to the cache instance:
/// Tauri state is keyed by type, so a single managed cache is the only way
/// for the command IPC and every running ACP session to share one snapshot
/// without introducing a near-empty wrapper type per provider. Each caller
/// already knows which dialect it speaks, and the key keeps one provider's
/// catalogue from ever being served for another.
#[derive(Debug, Default)]
pub struct AcpSlashCommandCache {
    inner: Mutex<HashMap<(AcpDialect, PathBuf), Vec<ProviderSlashCommand>>>,
}

impl AcpSlashCommandCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read the snapshot a running session already published, without
    /// probing.
    ///
    /// Agents that only announce their catalogue after `session/new` cannot
    /// be probed for it: a probe would have to create a real session on the
    /// agent side, which costs a child process and writes the agent's own
    /// session storage — far too side-effecting for populating a composer
    /// menu. Before any session has run for `cwd` the honest answer is an
    /// empty list, which the popup renders without a failure footer.
    pub async fn cached(&self, dialect: AcpDialect, cwd: &Path) -> Vec<ProviderSlashCommand> {
        self.inner
            .lock()
            .await
            .get(&(dialect, cwd.to_path_buf()))
            .cloned()
            .unwrap_or_default()
    }

    pub async fn get_or_harvest(
        &self,
        dialect: AcpDialect,
        binary_path: &Path,
        cwd: &Path,
    ) -> Result<Vec<ProviderSlashCommand>, String> {
        let key = (dialect, cwd.to_path_buf());
        // Scoped so the guard is dropped before the probe below.
        {
            let entries = self.inner.lock().await;
            if let Some(commands) = entries.get(&key) {
                return Ok(commands.clone());
            }
        }

        // The probe spawns a child process and can take seconds, so the lock
        // must NOT be held across it: live `available_commands_update`
        // notifications take the same lock on the session's notification task,
        // and stalling those stalls the whole stream for that session.
        let Some(initialized) = harvest_initialize(dialect, binary_path, cwd).await? else {
            return Ok(Vec::new());
        };
        let commands = available_commands_from_value(&initialized).unwrap_or_default();

        // Anything that landed while the probe ran is newer than the probe's
        // snapshot — a live session update, or another probe that finished
        // later — so it wins and our result is discarded.
        let mut entries = self.inner.lock().await;
        if let Some(existing) = entries.get(&key) {
            return Ok(existing.clone());
        }
        entries.insert(key, commands.clone());
        Ok(commands)
    }

    /// Apply one advertised full snapshot. An absent or malformed catalogue
    /// is ignored, while an explicitly empty array correctly clears it.
    pub async fn replace_from_value(
        &self,
        dialect: AcpDialect,
        cwd: &Path,
        value: &Value,
    ) -> bool {
        let Some(commands) = available_commands_from_value(value) else {
            return false;
        };
        self.inner
            .lock()
            .await
            .insert((dialect, cwd.to_path_buf()), commands);
        true
    }
}

/// Run the dialect's initialize-only probe, if it has one, and surface any
/// failure in the provider's own error vocabulary so the composer's footer
/// names the CLI the user actually has to install or log into.
///
/// `None` means the dialect announces its catalogue only over a live
/// session, so there is nothing a cheap probe could learn.
async fn harvest_initialize(
    dialect: AcpDialect,
    binary_path: &Path,
    cwd: &Path,
) -> Result<Option<Value>, String> {
    match dialect {
        AcpDialect::Cursor => Ok(None),
        AcpDialect::Grok => crate::agent_provider::grok::capabilities::harvest_grok_initialize(
            binary_path,
            Some(cwd.to_path_buf()),
            "codemux-command-harvest",
        )
        .await
        .map(Some)
        .map_err(|error| error.to_command_string()),
    }
}

/// Parse initialize metadata or a standard/xAI session-update envelope.
///
/// xAI builds have emitted both ACP camelCase fields and extension-style
/// snake_case fields, so both spellings are accepted. The returned vector is
/// already shaped for the existing provider-command GUI contract.
pub(crate) fn available_commands_from_value(value: &Value) -> Option<Vec<ProviderSlashCommand>> {
    let values = [
        "/_meta/availableCommands",
        "/_meta/available_commands",
        "/availableCommands",
        "/available_commands",
        "/update/availableCommands",
        "/update/available_commands",
        "/update/_meta/availableCommands",
        "/update/_meta/available_commands",
    ]
    .into_iter()
    .find_map(|pointer| value.pointer(pointer).and_then(Value::as_array))?;

    let mut seen = HashSet::new();
    let mut commands = Vec::new();
    for entry in values {
        let Some(name) = entry.get("name").and_then(Value::as_str) else {
            continue;
        };
        let name = name.trim().trim_start_matches('/');
        if name.is_empty() {
            continue;
        }
        let normalized = name.to_ascii_lowercase();
        if ACP_UNSAFE_COMMANDS.contains(&normalized.as_str()) || !seen.insert(normalized) {
            continue;
        }
        commands.push(ProviderSlashCommand {
            name: name.to_string(),
            description: string_field(entry, &["description"]).unwrap_or_default(),
            argument_hint: entry
                .pointer("/input/hint")
                .and_then(Value::as_str)
                .and_then(nonempty)
                .or_else(|| {
                    string_field(
                        entry,
                        &["inputHint", "input_hint", "argumentHint", "argument_hint"],
                    )
                })
                .unwrap_or_default(),
        });
    }
    Some(commands)
}

pub(crate) fn is_available_commands_update(value: &Value) -> bool {
    let update = value.get("update").unwrap_or(value);
    update
        .get("sessionUpdate")
        .or_else(|| update.get("session_update"))
        .or_else(|| update.get("type"))
        .and_then(Value::as_str)
        .is_some_and(|kind| {
            matches!(
                kind,
                "available_commands_update" | "availableCommandsUpdate"
            )
        })
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .and_then(nonempty)
}

fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_official_initialize_commands_and_filters_only_unsafe_command() {
        let commands = available_commands_from_value(&json!({
            "_meta": {
                "availableCommands": [{
                    "name": "/research",
                    "description": "Research a topic",
                    "input": { "hint": "<topic>" }
                }, {
                    "name": "always-approve",
                    "description": "Disable approval prompts"
                }, {
                    "name": "RESEARCH",
                    "description": "duplicate"
                }, {
                    "name": "context",
                    "description": "Show context"
                }]
            }
        }))
        .expect("advertised catalogue");

        assert_eq!(
            commands,
            vec![
                ProviderSlashCommand {
                    name: "research".into(),
                    description: "Research a topic".into(),
                    argument_hint: "<topic>".into(),
                },
                ProviderSlashCommand {
                    name: "context".into(),
                    description: "Show context".into(),
                    argument_hint: String::new(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn snake_case_live_update_replaces_the_full_snapshot() {
        let cache = AcpSlashCommandCache::new();
        let cwd = Path::new("/workspace");
        assert!(
            cache
                .replace_from_value(
                    AcpDialect::Grok,
                    cwd,
                    &json!({ "_meta": { "availableCommands": [{ "name": "old" }] } }),
                )
                .await
        );
        assert!(
            cache
                .replace_from_value(
                    AcpDialect::Grok,
                    cwd,
                    &json!({
                        "update": {
                            "sessionUpdate": "available_commands_update",
                            "available_commands": [{
                                "name": "new",
                                "argument_hint": "<value>"
                            }]
                        }
                    }),
                )
                .await
        );

        assert_eq!(
            cache.cached(AcpDialect::Grok, cwd).await,
            vec![ProviderSlashCommand {
                name: "new".into(),
                description: String::new(),
                argument_hint: "<value>".into(),
            }]
        );

        assert!(
            !cache
                .replace_from_value(AcpDialect::Grok, cwd, &json!({ "update": { "unrelated": [] } }))
                .await
        );
        assert_eq!(cache.cached(AcpDialect::Grok, cwd).await[0].name, "new");
    }

    #[tokio::test]
    async fn each_dialect_keeps_its_own_catalogue_for_the_same_cwd() {
        let cache = AcpSlashCommandCache::new();
        let cwd = Path::new("/workspace");
        cache
            .replace_from_value(
                AcpDialect::Cursor,
                cwd,
                &json!({
                    "update": {
                        "sessionUpdate": "available_commands_update",
                        "availableCommands": [{
                            "name": "cursor-only",
                            "description": "Cursor catalogue entry"
                        }]
                    }
                }),
            )
            .await;
        cache
            .replace_from_value(
                AcpDialect::Grok,
                cwd,
                &json!({ "_meta": { "availableCommands": [{ "name": "grok-only" }] } }),
            )
            .await;

        assert_eq!(
            cache.cached(AcpDialect::Cursor, cwd).await[0].name,
            "cursor-only"
        );
        assert_eq!(cache.cached(AcpDialect::Grok, cwd).await[0].name, "grok-only");
    }

    #[test]
    fn parses_a_standard_initialize_catalogue_without_vendor_metadata() {
        let commands = available_commands_from_value(&json!({
            "availableCommands": [{
                "name": "review",
                "description": "Review the diff",
                "input": { "hint": "<path>" }
            }]
        }))
        .expect("advertised catalogue");

        assert_eq!(
            commands,
            vec![ProviderSlashCommand {
                name: "review".into(),
                description: "Review the diff".into(),
                argument_hint: "<path>".into(),
            }]
        );
    }

    #[test]
    fn parses_update_entries_that_carry_no_argument_hint() {
        let commands = available_commands_from_value(&json!({
            "update": {
                "sessionUpdate": "available_commands_update",
                "availableCommands": [
                    { "name": "copy-request-id", "description": "Copy the request id" },
                    { "name": "plan", "description": "Draft a plan (project)" }
                ]
            }
        }))
        .expect("advertised catalogue");

        assert_eq!(
            commands,
            vec![
                ProviderSlashCommand {
                    name: "copy-request-id".into(),
                    description: "Copy the request id".into(),
                    argument_hint: String::new(),
                },
                ProviderSlashCommand {
                    name: "plan".into(),
                    description: "Draft a plan (project)".into(),
                    argument_hint: String::new(),
                },
            ]
        );
    }

    #[test]
    fn explicit_empty_snapshot_is_distinct_from_missing_metadata() {
        assert_eq!(
            available_commands_from_value(&json!({ "update": { "availableCommands": [] } })),
            Some(Vec::new())
        );
        assert_eq!(available_commands_from_value(&json!({})), None);
    }

    #[test]
    fn recognizes_standard_and_xai_update_discriminators() {
        assert!(is_available_commands_update(&json!({
            "update": { "sessionUpdate": "available_commands_update" }
        })));
        assert!(is_available_commands_update(&json!({
            "update": { "type": "availableCommandsUpdate" }
        })));
        assert!(!is_available_commands_update(&json!({
            "update": { "type": "turn_completed", "available_commands": [] }
        })));
    }
}
