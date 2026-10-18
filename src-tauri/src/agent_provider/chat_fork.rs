//! Persistent chat forks: the pure half of `commands::chat_fork`.
//!
//! A fork continues the provider's own session, so the model keeps its full
//! context, and then lives on as an independent chat:
//! * Claude: the sidecar's `fork-session` calls the agent SDK's
//!   `forkSession`, which copies the transcript into a new session file
//!   (`upToMessageId` cuts it at an earlier turn).
//! * Codex: a persistent `thread/fork` (the side chat's fork without
//!   `ephemeral`), then `thread/rollback` drops the turns after the cut.
//!
//! Earlier-turn cuts are addressed by the source row of the user message
//! that starts the first dropped turn. Codex resolves that through the turn
//! checkpoints, the same timeline turn revert trusts; Claude matches it
//! against the SDK transcript and refuses when the two disagree.

use serde::Serialize;
use serde_json::{json, Value};

use crate::database::AgentChatTurnCheckpointRecord;

/// What `agent_chat_fork_thread` reports: the new thread, ready to open.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatForkCreated {
    pub thread_id: String,
    pub provider: String,
    pub title: String,
    pub workspace_id: String,
    pub cwd: String,
}

/// The fork's title: the source's with a ` (fork)` suffix, matching what the
/// Claude SDK names its own forks.
pub fn fork_title(source_title: Option<&str>) -> String {
    let base = source_title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or("Chat");
    format!("{base} (fork)")
}

/// How many trailing Codex turns a fork cut before `before` drops: the
/// checkpointed turn whose user message is that row, and every turn after
/// it. A checkpoint's `transcript_cutoff_id` is the row just before its user
/// message (the last row a revert keeps).
pub fn codex_rollback_turns(
    checkpoints: &[AgentChatTurnCheckpointRecord],
    before: i64,
) -> Result<u32, String> {
    let position = checkpoints
        .iter()
        .position(|checkpoint| checkpoint.transcript_cutoff_id + 1 == before)
        .ok_or(
            "Codex can only fork from turns that have a checkpoint. Fork from the latest message instead.",
        )?;
    u32::try_from(checkpoints.len() - position).map_err(|_| "Too many turns to drop.".to_string())
}

/// The id of a persistent Codex fork. A fork Codex kept off disk could not
/// be resumed by the new chat, so it is refused.
pub fn codex_persistent_fork_id(response: &Value) -> Result<String, String> {
    let id = super::side_chat::codex_fork_thread_id(response)?;
    if response.pointer("/thread/ephemeral") == Some(&json!(true)) {
        return Err("Codex kept the fork off disk, so it can't be continued.".into());
    }
    Ok(id)
}

/// A fork RPC error, rephrased where the user can act on it.
pub fn fork_error(message: &str) -> String {
    if message.contains("no rollout found") || message.contains("not found") {
        "This chat has nothing to fork yet. Wait for the first reply, then fork it.".into()
    } else if message.contains("fork_point_not_found") {
        "Couldn't find that turn in the agent's own history. Fork from the latest message instead."
            .into()
    } else {
        format!("Couldn't fork this chat: {message}")
    }
}

/// An earlier-turn Claude cut: keep `keep_prompts` user prompts; the next
/// one should read `next_prompt`. `occurrence` counts the kept prompts with
/// exactly that text, so the sidecar can find it by text when the counts
/// disagree (a prompt the SDK never saw, such as one sent before a handoff).
#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeForkCut {
    pub keep_prompts: usize,
    pub next_prompt: String,
    pub occurrence: usize,
}

impl ClaudeForkCut {
    pub fn new(earlier_prompts: &[String], next_prompt: String) -> Self {
        let key = next_prompt.trim();
        Self {
            keep_prompts: earlier_prompts.len(),
            occurrence: earlier_prompts
                .iter()
                .filter(|prompt| prompt.trim() == key)
                .count(),
            next_prompt,
        }
    }
}

/// `fork-session` params for the Claude sidecar. `dir` is where the source
/// transcript lives; `target_dir` is where the fork will run.
pub fn claude_fork_session_params(
    session_id: &str,
    dir: &str,
    target_dir: &str,
    cut: Option<&ClaudeForkCut>,
) -> Value {
    let mut params = json!({
        "sessionId": session_id,
        "dir": dir,
        "targetDir": target_dir,
    });
    if let Some(cut) = cut {
        params["cut"] = json!({
            "keepPrompts": cut.keep_prompts,
            "nextPrompt": cut.next_prompt,
            "occurrence": cut.occurrence,
        });
    }
    params
}

/// The forked session id from a `fork-session` reply.
pub fn claude_fork_session_id(response: &Value) -> Result<String, String> {
    response
        .get("sessionId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "Claude returned a fork without a session id".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkpoint(turn_index: i64, cutoff: i64) -> AgentChatTurnCheckpointRecord {
        AgentChatTurnCheckpointRecord {
            thread_id: "t".into(),
            turn_index,
            workspace_id: "ws".into(),
            repo_path: "/repo".into(),
            client_nonce: None,
            transcript_cutoff_id: cutoff,
            ref_name: format!("refs/codemux/{turn_index}"),
            snapshot_commit: "s".into(),
            head_commit: "h".into(),
            branch: None,
            created_at: String::new(),
        }
    }

    #[test]
    fn titles_mark_the_fork() {
        assert_eq!(fork_title(Some("Fix the parser")), "Fix the parser (fork)");
        assert_eq!(fork_title(Some("  ")), "Chat (fork)");
        assert_eq!(fork_title(None), "Chat (fork)");
    }

    #[test]
    fn codex_cuts_drop_the_checkpointed_turn_and_everything_after() {
        // Cutoffs sit one row before each turn's user message.
        let timeline = [checkpoint(3, 29), checkpoint(4, 39), checkpoint(5, 49)];
        assert_eq!(codex_rollback_turns(&timeline, 50).unwrap(), 1);
        assert_eq!(codex_rollback_turns(&timeline, 30).unwrap(), 3);
        assert!(codex_rollback_turns(&timeline, 49).is_err(), "not a turn start");
        assert!(codex_rollback_turns(&[], 30).is_err());
    }

    #[test]
    fn codex_forks_must_be_persistent() {
        let ok = json!({ "thread": { "id": "fork-1", "ephemeral": false } });
        assert_eq!(codex_persistent_fork_id(&ok).unwrap(), "fork-1");
        assert!(codex_persistent_fork_id(&json!({ "thread": { "id": "f", "ephemeral": true } })).is_err());
        assert!(codex_persistent_fork_id(&json!({ "thread": {} })).is_err());
    }

    #[test]
    fn persistent_codex_fork_params_carry_no_side_chat_restrictions() {
        let params = super::super::side_chat::codex_fork_base_params("main", "/wt", &["docs".into()]);
        assert_eq!(params["threadId"], "main");
        assert_eq!(params["cwd"], "/wt");
        assert_eq!(params["config"]["mcp_servers"]["docs"]["enabled"], false);
        for key in ["ephemeral", "sandbox", "approvalPolicy", "developerInstructions"] {
            assert!(params.get(key).is_none(), "{key} belongs to side chats only");
        }
    }

    #[test]
    fn claude_cuts_count_prompts_and_repeats() {
        let earlier = vec!["go".to_string(), "continue".into(), " continue ".into()];
        let cut = ClaudeForkCut::new(&earlier, "continue".into());
        assert_eq!(cut.keep_prompts, 3);
        assert_eq!(cut.occurrence, 2);
        let params = claude_fork_session_params("s", "/repo", "/wt", Some(&cut));
        assert_eq!(params["cut"]["keepPrompts"], 3);
        assert_eq!(params["cut"]["nextPrompt"], "continue");
        assert_eq!(params["targetDir"], "/wt");
        let latest = claude_fork_session_params("s", "/repo", "/repo", None);
        assert!(latest.get("cut").is_none());
    }

    #[test]
    fn claude_replies_need_a_session_id() {
        assert_eq!(claude_fork_session_id(&json!({ "sessionId": "abc" })).unwrap(), "abc");
        assert!(claude_fork_session_id(&json!({ "sessionId": " " })).is_err());
        assert!(claude_fork_session_id(&json!({})).is_err());
    }

    #[test]
    fn fork_errors_explain_what_to_do() {
        assert!(fork_error("thread/fork failed: no rollout found").contains("Wait for the first reply"));
        assert!(fork_error("fork_point_not_found: x").contains("latest message"));
        assert_eq!(fork_error("boom"), "Couldn't fork this chat: boom");
    }
}
