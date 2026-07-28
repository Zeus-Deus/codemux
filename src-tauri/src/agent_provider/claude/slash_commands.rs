//! Live discovery of the deployed Claude Code CLI's slash commands.
//!
//! Codemux never hardcodes the CLI's command vocabulary — `/compact`,
//! `/init`, `/review`, custom `~/.claude/commands` entries, project
//! `.claude/commands` entries, … all come from the SDK's
//! `supportedCommands()` API via the sidecar's `list-commands`
//! JSON-RPC method (a transient `query()` probe, same lifecycle as
//! `list-models`). This is entirely data-driven, mirroring how a
//! reference multi-provider client populates its provider-command
//! menu from the SDK init result rather than a static table.
//!
//! Results are cached per working directory for the app's lifetime —
//! commands are cwd-sensitive because project-scoped custom commands
//! resolve relative to it. The composer's slash popup triggers the
//! harvest lazily on first open, so the CLI spawn cost is paid at
//! most once per project per run.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Mutex;

use crate::json_rpc_child::{JsonRpcChild, SpawnConfig};

/// One provider slash command, shaped for the frontend popup.
/// Serialised camelCase to match the TS `ProviderSlashCommand` type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSlashCommand {
    /// Command name without the leading slash (e.g. `compact`).
    pub name: String,
    /// One-line description from the SDK. May be empty.
    #[serde(default)]
    pub description: String,
    /// Argument hint (e.g. `<pr-url>`). May be empty.
    #[serde(default)]
    pub argument_hint: String,
}

/// Wire shape of the sidecar's `list-commands` response. The SDK's
/// `SlashCommand` fields arrive camelCase; unknown extras are ignored
/// so a future SDK addition doesn't break decode.
#[derive(Debug, Deserialize)]
struct SdkSlashCommand {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default, rename = "argumentHint")]
    argument_hint: String,
}

#[derive(Debug, Deserialize)]
struct ListCommandsResponse {
    #[serde(default)]
    commands: Vec<SdkSlashCommand>,
}

/// Process-wide cache of harvested command lists, keyed by cwd.
/// Mirrors `ClaudeCapabilityCache` — populated on first call per cwd
/// and reused for the rest of the app's lifetime.
#[derive(Default)]
pub struct ClaudeSlashCommandCache {
    inner: Mutex<HashMap<String, Vec<ProviderSlashCommand>>>,
}

impl ClaudeSlashCommandCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the cached list for `cwd` if present; otherwise run a
    /// live harvest, cache it, and return. Failures are NOT cached so
    /// a transient error (CLI not installed yet, spawn timeout)
    /// retries on the next popup open.
    pub async fn get_or_harvest(
        &self,
        cwd: &str,
    ) -> Result<Vec<ProviderSlashCommand>, String> {
        {
            let guard = self.inner.lock().await;
            if let Some(cached) = guard.get(cwd) {
                return Ok(cached.clone());
            }
        }
        let commands = harvest_via_sidecar(cwd).await?;
        let mut guard = self.inner.lock().await;
        guard.insert(cwd.to_string(), commands.clone());
        Ok(commands)
    }
}

/// Commands the CLI itself marks `isHidden` but which the SDK's
/// command list reports anyway. They are unusable from a Codemux chat
/// thread — each is either a debug affordance or gated on a session
/// kind Codemux never creates — so offering them is pure menu noise.
///
/// Verified against the deployed CLI's command registry: every name
/// here carries `isHidden` upstream, and the two `*workflow*` entries
/// describe their own gate ("server-launched sessions only",
/// "workflow_launch event sessions only").
///
/// This is deliberately a small, evidence-backed denylist rather than
/// a broad heuristic: a custom user command is never dropped by it.
const HIDDEN_COMMAND_NAMES: &[&str] = &[
    // Debug-only: writes a heap snapshot to the user's Desktop.
    "heapdump",
    // Gated on server-launched / workflow-handoff sessions.
    "workflow-launch-exec",
    // Consent flows that require the CLI's interactive UI.
    "design-consent",
    "design-revoke",
    // Renamed upstream to `usage-credits`, kept only as a hidden alias.
    "extra-usage",
    // Transient upsell states, surfaced by the CLI on its own terms.
    "pro-trial-expired",
    "rate-limit-options",
];

/// Whether a discovered command should be withheld from the menu.
///
/// Two rules, both evidence-backed:
/// 1. A `__` prefix is the CLI's internal-command convention
///    (e.g. `__remote-workflow`).
/// 2. An explicit denylist for hidden commands the SDK leaks anyway.
///
/// Note this filters only the *menu*. A user who types the name by
/// hand still has it forwarded verbatim — Codemux does not block
/// commands, it just declines to advertise unusable ones.
fn is_internal_command(name: &str) -> bool {
    if name.starts_with("__") {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    HIDDEN_COMMAND_NAMES.contains(&lower.as_str())
}

/// Case-insensitive dedupe by name, first occurrence wins. The SDK
/// can report a user-level and a project-level custom command with
/// the same name; the CLI resolves to one of them, so the menu should
/// list it once.
///
/// Internal / CLI-hidden commands are dropped here too, so every
/// consumer of the cache sees the same filtered vocabulary.
fn dedupe_commands(commands: Vec<SdkSlashCommand>) -> Vec<ProviderSlashCommand> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(commands.len());
    for c in commands {
        let name = c.name.trim().trim_start_matches('/').to_string();
        if name.is_empty() {
            continue;
        }
        if is_internal_command(&name) {
            continue;
        }
        if !seen.insert(name.to_ascii_lowercase()) {
            continue;
        }
        out.push(ProviderSlashCommand {
            name,
            description: c.description,
            argument_hint: c.argument_hint,
        });
    }
    out
}

/// Live-harvest the slash-command list by sending a `list-commands`
/// JSON-RPC request to a transient claude-agent sidecar. Same spawn /
/// timeout / shutdown lifecycle as `capabilities::harvest_via_sidecar`.
async fn harvest_via_sidecar(cwd: &str) -> Result<Vec<ProviderSlashCommand>, String> {
    let sidecar = super::sidecar_path::resolve_sidecar_path()
        .map_err(|e| format!("resolve sidecar: {e:?}"))?;
    let claude_binary = which::which("claude").map_err(|_| {
        "claude binary not on PATH (install Claude Code or sign in to it)".to_string()
    })?;

    let child = tokio::time::timeout(
        Duration::from_secs(20),
        JsonRpcChild::spawn(SpawnConfig {
            program: sidecar,
            args: vec![],
            env: HashMap::new(),
            cwd: None,
            default_timeout: Duration::from_secs(20),
        }),
    )
    .await
    .map_err(|_| "sidecar spawn timed out".to_string())?
    .map_err(|e| format!("sidecar spawn: {e}"))?;

    let response = child
        .request(
            "list-commands",
            json!({
                "cwd": cwd,
                "pathToClaudeCodeExecutable": claude_binary.to_string_lossy(),
            }),
        )
        .await;
    let _ = child.shutdown().await;
    let response = response.map_err(|e| format!("list-commands RPC: {e}"))?;

    let parsed: ListCommandsResponse =
        serde_json::from_value(response).map_err(|e| format!("decode: {e}"))?;

    Ok(dedupe_commands(parsed.commands))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sdk(name: &str, description: &str, hint: &str) -> SdkSlashCommand {
        SdkSlashCommand {
            name: name.into(),
            description: description.into(),
            argument_hint: hint.into(),
        }
    }

    #[test]
    fn dedupe_is_case_insensitive_first_wins() {
        let out = dedupe_commands(vec![
            sdk("compact", "Compact the conversation", ""),
            sdk("Compact", "duplicate", ""),
            sdk("review", "Review a PR", "<pr-url>"),
        ]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].name, "compact");
        assert_eq!(out[0].description, "Compact the conversation");
        assert_eq!(out[1].name, "review");
        assert_eq!(out[1].argument_hint, "<pr-url>");
    }

    #[test]
    fn dedupe_strips_leading_slash_and_drops_empty_names() {
        let out = dedupe_commands(vec![
            sdk("/init", "Initialise CLAUDE.md", ""),
            sdk("", "nameless", ""),
            sdk("   ", "blank", ""),
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "init");
    }

    #[test]
    fn drops_double_underscore_internal_commands() {
        let out = dedupe_commands(vec![
            sdk("compact", "Compact the conversation", ""),
            sdk(
                "__remote-workflow",
                "Run the workflow script delivered in this session environment",
                "",
            ),
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "compact");
    }

    #[test]
    fn drops_cli_hidden_commands_the_sdk_still_reports() {
        // Every name here is `isHidden` in the deployed CLI registry
        // but comes back from the SDK's command list regardless.
        let out = dedupe_commands(vec![
            sdk("heapdump", "Dump the JS heap to ~/Desktop", ""),
            sdk("workflow-launch-exec", "Execute a handoff", ""),
            sdk("design-consent", "Grant access", ""),
            sdk("design-revoke", "Revoke access", ""),
            sdk("extra-usage", "Renamed to /usage-credits", ""),
            sdk("usage-credits", "Configure usage credits", ""),
        ]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "usage-credits");
    }

    #[test]
    fn hidden_filter_is_case_insensitive_and_slash_tolerant() {
        let out = dedupe_commands(vec![sdk("/HeapDump", "shouting", "")]);
        assert!(out.is_empty());
    }

    #[test]
    fn keeps_user_commands_that_merely_resemble_hidden_ones() {
        // The denylist is exact-match: a custom command must never be
        // swallowed by a prefix/substring heuristic.
        let out = dedupe_commands(vec![
            sdk("design", "Real user command", ""),
            sdk("heapdump-report", "Custom command", ""),
            sdk("my__internal", "Underscores not at the start", ""),
        ]);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn decode_tolerates_missing_optional_fields() {
        let parsed: ListCommandsResponse = serde_json::from_value(json!({
            "commands": [
                { "name": "compact" },
                { "name": "review", "description": "Review", "argumentHint": "<url>" }
            ]
        }))
        .expect("decodes");
        let out = dedupe_commands(parsed.commands);
        assert_eq!(out[0].description, "");
        assert_eq!(out[1].argument_hint, "<url>");
    }
}
