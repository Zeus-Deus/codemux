//! TUI actions with a real app-server equivalent. Do not send these as prompts.

use serde_json::{json, Value};

use crate::agent_provider::claude::slash_commands::ProviderSlashCommand;
use crate::agent_provider::slash_commands::leading_command;

pub fn commands() -> Vec<ProviderSlashCommand> {
    vec![
        ProviderSlashCommand {
            name: "compact".into(),
            description: "Summarize conversation history to free up context".into(),
            argument_hint: String::new(),
        },
        ProviderSlashCommand {
            name: "review".into(),
            description: "Review changes, a branch, a commit, or custom instructions".into(),
            argument_hint: "[uncommitted | branch <name> | commit <sha> | instructions]".into(),
        },
    ]
}

pub fn request(text: &str, thread_id: &str) -> Result<Option<(&'static str, Value)>, String> {
    let Some((name, arguments)) = leading_command(text) else {
        return Ok(None);
    };
    match name.to_ascii_lowercase().as_str() {
        "compact" => {
            if !arguments.is_empty() {
                return Err("Codex /compact does not accept arguments. Send instructions in a separate message.".into());
            }
            Ok(Some((
                "thread/compact/start",
                json!({ "threadId": thread_id }),
            )))
        }
        "review" => {
            let (kind, value) = arguments
                .split_once(char::is_whitespace)
                .unwrap_or((arguments, ""));
            let value = value.trim();
            let target = match kind {
                "" | "uncommitted" if value.is_empty() => json!({ "type": "uncommittedChanges" }),
                "branch" if !value.is_empty() => json!({ "type": "baseBranch", "branch": value }),
                "commit" if !value.is_empty() => json!({ "type": "commit", "sha": value }),
                "branch" | "commit" => {
                    return Err(format!("Codex /review {kind} requires an argument."))
                }
                _ => json!({ "type": "custom", "instructions": arguments }),
            };
            Ok(Some((
                "review/start",
                json!({ "threadId": thread_id, "delivery": "inline", "target": target }),
            )))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_native_commands_use_rpc_targets() {
        let (method, body) = request("/compact", "thread-1").unwrap().unwrap();
        assert_eq!(method, "thread/compact/start");
        assert_eq!(body, json!({"threadId": "thread-1"}));
        for (text, target) in [
            ("/review", json!({"type": "uncommittedChanges"})),
            (
                "/review branch main",
                json!({"type": "baseBranch", "branch": "main"}),
            ),
            (
                "/review commit abc123",
                json!({"type": "commit", "sha": "abc123"}),
            ),
            (
                "/review check security",
                json!({"type": "custom", "instructions": "check security"}),
            ),
        ] {
            let (method, body) = request(text, "thread-1").unwrap().unwrap();
            assert_eq!(method, "review/start");
            assert_eq!(body["target"], target);
        }
        assert!(request("/compact instructions", "t").is_err());
        assert!(request("/review branch", "t").is_err());
        assert!(request("/home/file", "t").unwrap().is_none());
    }
}
