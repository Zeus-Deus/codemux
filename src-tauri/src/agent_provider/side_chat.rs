//! `/btw` side chats: an ephemeral, read-only fork of a chat's provider
//! session that answers a side question without touching the main thread.
//!
//! A side chat owns its own provider process (a dedicated Claude sidecar or
//! `codex app-server`), so it can run while the main turn keeps streaming and
//! closing it disposes everything it started. Nothing here is written to the
//! chat database or routed through the main event bridge: the fork never
//! appears in the sidebar, history or transcript.
//!
//! Wire shapes were confirmed against the installed CLIs (codex-cli 0.162.1,
//! Claude Code 2.1.299 with agent SDK 0.2.114):
//! * Codex `thread/fork` with `ephemeral: true` returns `thread.path: null`
//!   and writes no rollout; `thread/archive` then fails ("no rollout found"),
//!   so disposal is `thread/unsubscribe` plus process shutdown.
//! * Claude `resume` + `forkSession: true` + `persistSession: false` keeps
//!   the context, writes no transcript file and leaves the source transcript
//!   byte-identical; follow-ups continue on the same live streaming query.

use serde::Serialize;
use serde_json::{json, Value};

use super::types::ProviderKind;

/// One update from a running side chat, forwarded to the renderer.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SideChatUpdate {
    /// Streaming answer text for the current turn.
    Delta { text: String },
    /// The current turn finished. `text` is the provider's final answer
    /// when it reported one (used if no deltas arrived).
    TurnCompleted { text: Option<String> },
    /// The current turn (or the whole fork) failed.
    Failed { message: String },
    /// The fork's process is gone; no further turns are possible.
    Closed,
}

/// What `agent_chat_side_chat_start` reports back: the model and reasoning
/// effort the fork actually runs with, so the pane can say which model is
/// answering. `None` means the provider's own default.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SideChatStarted {
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// A stored model/effort value, or `None` when it is blank.
pub fn non_blank(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Renderer payload for the `agent-chat-side-chat` event.
#[derive(Debug, Clone, Serialize)]
pub struct SideChatEventPayload {
    pub side_id: String,
    #[serde(flatten)]
    pub update: SideChatUpdate,
}

pub const SIDE_CHAT_EVENT: &str = "agent-chat-side-chat";

/// Instructions given to the fork so it answers instead of continuing work.
pub const SIDE_CHAT_INSTRUCTIONS: &str = "You are in a side chat. The user is asking a quick side question about the conversation so far while the main session keeps working. Answer only that question, directly and concisely. This side chat is read-only: do not edit files, run commands that change anything, or continue the main task. The main session may still be running, so the transcript you see can stop mid-task and its work may already be further along: never take over, finish or report on the main task's unfinished steps unless the user asks about them. Use only the read-only tools you have; if one is unavailable, answer from what you can read. Nothing you say here is added to the main conversation.";

/// Whether `provider` can fork its own session natively. Shared by `/btw`
/// side chats and persistent chat forks.
pub fn supports_native_fork(provider: ProviderKind) -> bool {
    matches!(provider, ProviderKind::Claude | ProviderKind::Codex)
}

/// Whether `provider` can fork a side chat. Refusals are user-facing.
pub fn ensure_side_chat_supported(provider: ProviderKind) -> Result<(), String> {
    if supports_native_fork(provider) {
        return Ok(());
    }
    Err(format!(
        "Side chat isn't available for {} yet.",
        provider_label(provider)
    ))
}

pub fn provider_label(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Claude => "Claude",
        ProviderKind::Codex => "Codex",
        ProviderKind::Cursor => "Cursor",
        ProviderKind::Grok => "Grok",
        ProviderKind::Hermes => "Hermes",
        ProviderKind::Acp => "custom agents",
        ProviderKind::OpenCode => "OpenCode",
    }
}

/// The question text that starts or continues a side chat. Empty input is
/// refused before any process starts.
pub fn side_chat_question(text: &str) -> Result<String, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("Type a question after /btw.".into());
    }
    Ok(trimmed.to_string())
}

// ── Codex ───────────────────────────────────────────────────────────────

/// Launch overrides for the side app-server. Lifecycle hooks, apps and
/// plugins are switched off so the fork cannot run user automation or load
/// plugin-provided MCP servers; subagents and web search are off so it stays
/// a short read-only answer.
pub fn codex_side_chat_args() -> Vec<String> {
    let mut args = Vec::new();
    for feature in ["hooks", "apps", "plugins", "multi_agent", "multi_agent_v2"] {
        args.extend(["-c".to_string(), format!("features.{feature}=false")]);
    }
    args.extend([
        "-c".to_string(),
        "agents.enabled=false".to_string(),
        "-c".to_string(),
        "web_search=\"disabled\"".to_string(),
    ]);
    args.push("app-server".into());
    args
}

/// Names of the MCP servers the side app-server would start, from its
/// `config/read` response.
pub fn codex_enabled_mcp_servers(config: &Value) -> Vec<String> {
    let Some(servers) = config.get("mcp_servers").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut names: Vec<String> = servers
        .iter()
        .filter(|(_, server)| server.get("enabled") != Some(&json!(false)))
        .map(|(name, _)| name.clone())
        .collect();
    names.sort();
    names
}

/// The `thread/fork` params every fork shares: the source thread, the fork's
/// working directory, no turns echoed back, and the named MCP servers left
/// unstarted in the forking process (a nested override; the dotted-key form
/// is rejected by Codex 0.162).
pub fn codex_fork_base_params(thread_id: &str, cwd: &str, mcp_servers: &[String]) -> Value {
    let mut params = json!({
        "threadId": thread_id,
        "excludeTurns": true,
        "cwd": cwd,
    });
    if !mcp_servers.is_empty() {
        let disabled: serde_json::Map<String, Value> = mcp_servers
            .iter()
            .map(|name| (name.clone(), json!({ "enabled": false })))
            .collect();
        params["config"] = json!({ "mcp_servers": disabled });
    }
    params
}

/// `thread/fork` params: an ephemeral (never written to disk) read-only fork
/// that can never ask for approval, with its MCP servers disabled.
pub fn codex_fork_params(main_thread_id: &str, cwd: &str, mcp_servers: &[String]) -> Value {
    let mut params = codex_fork_base_params(main_thread_id, cwd, mcp_servers);
    params["ephemeral"] = json!(true);
    params["sandbox"] = json!("read-only");
    params["approvalPolicy"] = json!("never");
    params["developerInstructions"] = json!(SIDE_CHAT_INSTRUCTIONS);
    params
}

/// `turn/start` params for one side-chat turn, re-asserting the read-only
/// sandbox and no-approval policy on every turn. `model` and `effort` are the
/// main thread's, sent per turn exactly as the main Codex session sends them
/// (`TurnStartParams.model` / `.effort`), so the side answer is given by the
/// same model at the same reasoning effort. No collaboration mode is sent:
/// its developer instructions would replace the side-chat instructions the
/// fork was started with.
pub fn codex_turn_params(
    fork_thread_id: &str,
    text: &str,
    model: Option<&str>,
    effort: Option<&str>,
) -> Value {
    let mut params = json!({
        "threadId": fork_thread_id,
        "approvalPolicy": "never",
        "sandboxPolicy": { "type": "readOnly", "networkAccess": false },
        "input": [{ "type": "text", "text": text, "text_elements": [] }],
    });
    if let Some(model) = non_blank(model) {
        params["model"] = json!(model);
    }
    if let Some(effort) = non_blank(effort) {
        params["effort"] = json!(effort);
    }
    params
}

/// The model and effort a Codex fork runs with: the main thread's stored
/// choice when it has one (sent on every turn), otherwise what `thread/fork`
/// reported it resolved from the user's Codex config.
pub fn codex_fork_settings(
    fork_response: &Value,
    model: Option<&str>,
    effort: Option<&str>,
) -> SideChatStarted {
    let reported = |key: &str| non_blank(fork_response.get(key).and_then(Value::as_str));
    SideChatStarted {
        model: non_blank(model).or_else(|| reported("model")),
        effort: non_blank(effort).or_else(|| reported("reasoningEffort")),
    }
}

/// The reply to a server-initiated request on the fork. Everything that
/// would act is declined; Codemux host tools are not served to side chats.
pub fn codex_side_request_reply(method: &str) -> Result<Value, (i64, String)> {
    match method {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            Ok(json!({ "decision": "decline" }))
        }
        "execCommandApproval" | "applyPatchApproval" => Ok(json!({ "decision": "denied" })),
        "item/permissions/requestApproval" => Ok(json!({ "permissions": [] })),
        "mcpServer/elicitation/request" => Ok(json!({ "action": "decline" })),
        "item/tool/call" => Ok(json!({
            "success": false,
            "contentItems": [{ "type": "inputText", "text": "Codemux tools are unavailable in a read-only side chat." }],
        })),
        other => Err((
            -32601,
            format!("{other} is unavailable in a read-only side chat"),
        )),
    }
}

/// Map one fork notification to a side-chat update. Notifications for other
/// threads (the app-server can multiplex) are ignored.
pub fn codex_side_update(
    fork_thread_id: &str,
    method: &str,
    params: &Value,
) -> Option<SideChatUpdate> {
    if params.get("threadId").and_then(Value::as_str) != Some(fork_thread_id) {
        return None;
    }
    match method {
        "item/agentMessage/delta" => params
            .get("delta")
            .and_then(Value::as_str)
            .filter(|delta| !delta.is_empty())
            .map(|text| SideChatUpdate::Delta {
                text: text.to_string(),
            }),
        "turn/completed" => {
            let turn = params.get("turn").unwrap_or(&Value::Null);
            match turn.get("status").and_then(Value::as_str) {
                Some("failed") => Some(SideChatUpdate::Failed {
                    message: turn
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("The side chat turn failed.")
                        .to_string(),
                }),
                Some("interrupted") => Some(SideChatUpdate::Failed {
                    message: "The side chat turn was interrupted.".into(),
                }),
                _ => {
                    // The final agent messages, used only when no deltas
                    // streamed (the renderer prefers what it already showed).
                    let text: Vec<&str> = turn
                        .get("items")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter(|item| {
                            item.get("type").and_then(Value::as_str) == Some("agentMessage")
                        })
                        .filter_map(|item| item.get("text").and_then(Value::as_str))
                        .collect();
                    Some(SideChatUpdate::TurnCompleted {
                        text: (!text.is_empty()).then(|| text.join("\n\n")),
                    })
                }
            }
        }
        _ => None,
    }
}

/// Verify the fork Codex actually created is the read-only, ephemeral one we
/// asked for, and return its thread id. A server that ignored either
/// override must not run the side question.
pub fn codex_validate_fork(response: &Value) -> Result<String, String> {
    let id = codex_fork_thread_id(response)?;
    if response.pointer("/thread/ephemeral") != Some(&json!(true)) {
        return Err("Codex did not keep the side chat off disk".into());
    }
    if response.pointer("/sandbox/type").and_then(Value::as_str) != Some("readOnly") {
        return Err("Codex did not apply the read-only sandbox to the side chat".into());
    }
    if response.get("approvalPolicy").and_then(Value::as_str) != Some("never") {
        return Err("Codex did not disable approvals for the side chat".into());
    }
    Ok(id)
}

/// The id of the thread a `thread/fork` response created.
pub fn codex_fork_thread_id(response: &Value) -> Result<String, String> {
    let thread = response
        .get("thread")
        .ok_or("Codex returned no forked thread")?;
    thread
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "Codex returned a fork without an id".to_string())
}

/// Renderer-minted side-chat ids are UUIDs; anything else is refused so the
/// id can't smuggle text into event names or logs.
pub fn is_valid_side_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// A fork RPC error, rephrased where the cause is something the user can act
/// on. Codex can only fork a thread whose rollout exists (after a turn).
pub fn codex_fork_error(message: &str) -> String {
    if message.contains("no rollout found") {
        "This chat has nothing to fork yet. Send a message first, then ask your side question."
            .into()
    } else {
        format!("Couldn't fork this chat: {message}")
    }
}

// ── Claude ──────────────────────────────────────────────────────────────

/// `start-side-chat` params for the Claude sidecar. `effort` is the main
/// thread's stored level; the sidecar applies it like a main session does.
pub fn claude_side_chat_params(
    side_id: &str,
    cwd: &str,
    resume: &str,
    claude_binary: &str,
    model: Option<&str>,
    effort: Option<&str>,
    text: &str,
) -> Value {
    let mut params = json!({
        "sideId": side_id,
        "cwd": cwd,
        "resume": resume,
        "pathToClaudeCodeExecutable": claude_binary,
        "text": text,
        "appendSystemPrompt": SIDE_CHAT_INSTRUCTIONS,
    });
    if let Some(model) = model {
        params["model"] = json!(model);
    }
    if let Some(effort) = non_blank(effort) {
        params["effort"] = json!(effort);
    }
    params
}

/// Map one Claude sidecar notification to a side-chat update.
pub fn claude_side_update(side_id: &str, method: &str, params: &Value) -> Option<SideChatUpdate> {
    if params.get("sideId").and_then(Value::as_str) != Some(side_id) {
        return None;
    }
    let text = || {
        params
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    match method {
        "side-chat-delta" => text()
            .filter(|t| !t.is_empty())
            .map(|text| SideChatUpdate::Delta { text }),
        "side-chat-turn-completed" => Some(SideChatUpdate::TurnCompleted { text: text() }),
        "side-chat-error" => Some(SideChatUpdate::Failed {
            message: claude_side_error(&text().unwrap_or_default()),
        }),
        "side-chat-ended" => Some(SideChatUpdate::Closed),
        _ => None,
    }
}

/// The SDK reports a missing source transcript as a stale resume.
pub fn claude_side_error(message: &str) -> String {
    if message.contains("No conversation found with session ID") {
        "This chat's Claude session isn't on disk yet. Wait for the first reply, then try again."
            .into()
    } else if message.trim().is_empty() {
        "The side chat failed.".into()
    } else {
        message.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_claude_and_codex_fork_side_chats() {
        assert!(ensure_side_chat_supported(ProviderKind::Claude).is_ok());
        assert!(ensure_side_chat_supported(ProviderKind::Codex).is_ok());
        for (provider, label) in [
            (ProviderKind::Cursor, "Cursor"),
            (ProviderKind::Grok, "Grok"),
            (ProviderKind::OpenCode, "OpenCode"),
            (ProviderKind::Hermes, "Hermes"),
            (ProviderKind::Acp, "custom agents"),
        ] {
            assert_eq!(
                ensure_side_chat_supported(provider).unwrap_err(),
                format!("Side chat isn't available for {label} yet.")
            );
        }
    }

    #[test]
    fn empty_questions_are_refused() {
        assert!(side_chat_question("  \n").is_err());
        assert_eq!(side_chat_question("  why? ").unwrap(), "why?");
    }

    #[test]
    fn codex_fork_is_ephemeral_read_only_and_never_asks() {
        let params = codex_fork_params("main-1", "/repo", &[]);
        assert_eq!(params["threadId"], "main-1");
        assert_eq!(params["ephemeral"], true);
        assert_eq!(params["sandbox"], "read-only");
        assert_eq!(params["approvalPolicy"], "never");
        assert_eq!(params["cwd"], "/repo");
        assert!(params.get("config").is_none());
    }

    #[test]
    fn codex_fork_disables_every_mcp_server_with_nested_overrides() {
        let params = codex_fork_params("m", "/r", &["docs".into(), "node \"repl\"".into()]);
        assert_eq!(
            params["config"]["mcp_servers"]["docs"],
            json!({"enabled": false})
        );
        assert_eq!(
            params["config"]["mcp_servers"]["node \"repl\""],
            json!({"enabled": false})
        );
        // No dotted keys: Codex 0.162 rejects `mcp_servers."x".enabled`.
        assert!(params["config"]
            .as_object()
            .unwrap()
            .keys()
            .all(|k| !k.contains('.')));
    }

    #[test]
    fn codex_enabled_servers_skip_disabled_entries() {
        let config = json!({"mcp_servers": {
            "b": {"command": "x"}, "a": {"url": "y"}, "off": {"enabled": false}
        }});
        assert_eq!(codex_enabled_mcp_servers(&config), vec!["a", "b"]);
        assert!(codex_enabled_mcp_servers(&json!({})).is_empty());
    }

    #[test]
    fn codex_side_server_disables_automation() {
        let args = codex_side_chat_args();
        assert_eq!(args.last().map(String::as_str), Some("app-server"));
        for needle in [
            "features.hooks=false",
            "features.plugins=false",
            "features.apps=false",
            "agents.enabled=false",
        ] {
            assert!(args.iter().any(|a| a == needle), "missing {needle}");
        }
    }

    #[test]
    fn codex_turns_reassert_read_only_sandbox() {
        let params = codex_turn_params("fork-1", "what changed?", None, None);
        assert_eq!(params["threadId"], "fork-1");
        assert_eq!(params["approvalPolicy"], "never");
        assert_eq!(params["sandboxPolicy"]["type"], "readOnly");
        assert_eq!(params["sandboxPolicy"]["networkAccess"], false);
        assert_eq!(params["input"][0]["text"], "what changed?");
        assert!(params.get("model").is_none());
        assert!(params.get("effort").is_none());
        assert!(params.get("collaborationMode").is_none());
    }

    #[test]
    fn codex_turns_carry_the_main_threads_model_and_effort() {
        let params = codex_turn_params("fork-1", "q", Some("gpt-5.6-sol"), Some("high"));
        assert_eq!(params["model"], "gpt-5.6-sol");
        assert_eq!(params["effort"], "high");
        // Blank stored values fall through to Codex's own default.
        let blank = codex_turn_params("fork-1", "q", Some(" "), Some(""));
        assert!(blank.get("model").is_none());
        assert!(blank.get("effort").is_none());
    }

    #[test]
    fn codex_fork_settings_prefer_the_stored_choice() {
        let fork = json!({"model": "gpt-5.5", "reasoningEffort": "medium"});
        assert_eq!(
            codex_fork_settings(&fork, Some("gpt-5.6-sol"), Some("xhigh")),
            SideChatStarted {
                model: Some("gpt-5.6-sol".into()),
                effort: Some("xhigh".into())
            }
        );
        assert_eq!(
            codex_fork_settings(&fork, None, None),
            SideChatStarted {
                model: Some("gpt-5.5".into()),
                effort: Some("medium".into())
            }
        );
        assert_eq!(
            codex_fork_settings(&json!({}), None, None),
            SideChatStarted::default()
        );
    }

    #[test]
    fn codex_side_requests_never_grant_anything() {
        assert_eq!(
            codex_side_request_reply("item/commandExecution/requestApproval").unwrap()["decision"],
            "decline"
        );
        assert_eq!(
            codex_side_request_reply("item/fileChange/requestApproval").unwrap()["decision"],
            "decline"
        );
        assert_eq!(
            codex_side_request_reply("execCommandApproval").unwrap()["decision"],
            "denied"
        );
        assert_eq!(
            codex_side_request_reply("applyPatchApproval").unwrap()["decision"],
            "denied"
        );
        assert_eq!(
            codex_side_request_reply("item/tool/call").unwrap()["success"],
            false
        );
        assert_eq!(
            codex_side_request_reply("item/permissions/requestApproval").unwrap()["permissions"],
            json!([])
        );
        assert!(codex_side_request_reply("item/tool/requestUserInput").is_err());
    }

    #[test]
    fn codex_updates_are_scoped_to_the_fork_thread() {
        let delta = json!({"threadId": "fork", "delta": "Hi"});
        assert_eq!(
            codex_side_update("fork", "item/agentMessage/delta", &delta),
            Some(SideChatUpdate::Delta { text: "Hi".into() })
        );
        let other = json!({"threadId": "main", "delta": "leak"});
        assert_eq!(
            codex_side_update("fork", "item/agentMessage/delta", &other),
            None
        );
        let done = json!({"threadId": "fork", "turn": {"status": "completed"}});
        assert_eq!(
            codex_side_update("fork", "turn/completed", &done),
            Some(SideChatUpdate::TurnCompleted { text: None })
        );
        let failed = json!({"threadId": "fork", "turn": {"status": "failed", "error": {"message": "quota"}}});
        assert_eq!(
            codex_side_update("fork", "turn/completed", &failed),
            Some(SideChatUpdate::Failed {
                message: "quota".into()
            })
        );
    }

    #[test]
    fn codex_fork_response_must_prove_read_only_and_ephemeral() {
        let ok = json!({
            "thread": {"id": "fork-1", "ephemeral": true, "path": null},
            "sandbox": {"type": "readOnly", "networkAccess": false},
            "approvalPolicy": "never",
        });
        assert_eq!(codex_validate_fork(&ok).unwrap(), "fork-1");
        let mut writable = ok.clone();
        writable["sandbox"]["type"] = json!("workspaceWrite");
        assert!(codex_validate_fork(&writable).is_err());
        let mut persisted = ok.clone();
        persisted["thread"]["ephemeral"] = json!(false);
        assert!(codex_validate_fork(&persisted).is_err());
        let mut asks = ok.clone();
        asks["approvalPolicy"] = json!("on-request");
        assert!(codex_validate_fork(&asks).is_err());
    }

    #[test]
    fn side_ids_are_plain_tokens() {
        assert!(is_valid_side_id("3f2a1c9e-1b2c-4d5e-8f90-123456789abc"));
        assert!(!is_valid_side_id(""));
        assert!(!is_valid_side_id("../x"));
        assert!(!is_valid_side_id(&"a".repeat(65)));
    }

    #[test]
    fn codex_completion_falls_back_to_final_agent_message() {
        let done = json!({"threadId": "f", "turn": {"status": "completed", "items": [
            {"type": "userMessage", "text": "q"},
            {"type": "agentMessage", "text": "the answer"}
        ]}});
        assert_eq!(
            codex_side_update("f", "turn/completed", &done),
            Some(SideChatUpdate::TurnCompleted {
                text: Some("the answer".into())
            })
        );
    }

    #[test]
    fn codex_unforkable_thread_gets_an_actionable_message() {
        assert!(
            codex_fork_error("no rollout found for thread id x").contains("Send a message first")
        );
        assert!(codex_fork_error("boom").contains("boom"));
    }

    #[test]
    fn claude_params_fork_the_main_session() {
        let params = claude_side_chat_params(
            "s1",
            "/repo",
            "sdk-1",
            "claude",
            Some("opus"),
            Some("high"),
            "q",
        );
        assert_eq!(params["resume"], "sdk-1");
        assert_eq!(params["sideId"], "s1");
        assert_eq!(params["model"], "opus");
        assert_eq!(params["effort"], "high");
        assert_eq!(params["appendSystemPrompt"], SIDE_CHAT_INSTRUCTIONS);
        let bare = claude_side_chat_params("s", "/r", "x", "c", None, None, "q");
        assert!(bare.get("model").is_none());
        assert!(bare.get("effort").is_none());
        assert!(claude_side_chat_params("s", "/r", "x", "c", None, Some("  "), "q")
            .get("effort")
            .is_none());
    }

    #[test]
    fn started_info_serializes_for_the_renderer() {
        let started = SideChatStarted {
            model: Some("claude-opus-5-5".into()),
            effort: None,
        };
        assert_eq!(
            serde_json::to_value(started).unwrap(),
            json!({"model": "claude-opus-5-5", "effort": null})
        );
    }

    #[test]
    fn claude_updates_map_sidecar_notifications() {
        let p = |text: &str| json!({"sideId": "s1", "text": text});
        assert_eq!(
            claude_side_update("s1", "side-chat-delta", &p("Hel")),
            Some(SideChatUpdate::Delta { text: "Hel".into() })
        );
        assert_eq!(claude_side_update("s2", "side-chat-delta", &p("x")), None);
        assert_eq!(
            claude_side_update("s1", "side-chat-turn-completed", &p("Hello")),
            Some(SideChatUpdate::TurnCompleted {
                text: Some("Hello".into())
            })
        );
        assert_eq!(
            claude_side_update("s1", "side-chat-ended", &json!({"sideId": "s1"})),
            Some(SideChatUpdate::Closed)
        );
        let stale = claude_side_update(
            "s1",
            "side-chat-error",
            &p("No conversation found with session ID: abc"),
        );
        assert!(
            matches!(stale, Some(SideChatUpdate::Failed { message }) if message.contains("isn't on disk yet"))
        );
    }

    #[test]
    fn updates_serialize_with_a_kind_tag() {
        let payload = SideChatEventPayload {
            side_id: "s".into(),
            update: SideChatUpdate::Delta { text: "x".into() },
        };
        assert_eq!(
            serde_json::to_value(payload).unwrap(),
            json!({"side_id": "s", "kind": "delta", "text": "x"})
        );
    }
}
