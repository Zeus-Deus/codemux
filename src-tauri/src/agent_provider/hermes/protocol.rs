//! Wire helpers for the standard ACP surface Hermes speaks.
//!
//! Everything here is a pure function over `serde_json::Value`, for two
//! reasons. The first is the usual one: ACP payloads are open-ended, and
//! decoding a whole response into a struct means a newly-added key can
//! fail a session start that would otherwise have worked. The second is
//! specific to this adapter — the live protocol capture is the
//! specification, so each helper is written against an observed frame and
//! pinned by a test built from that frame rather than from spec prose.
//!
//! The Cursor adapter's `protocol.rs` is deliberately NOT the model here:
//! it is almost entirely `session/set_config_option`, a Cursor extension
//! with no counterpart in ACP. Hermes reports its models and modes on the
//! `session/new` response and takes changes through `session/set_model`
//! and `session/set_mode`, so none of that layer carries over.

use serde_json::{json, Value};

use crate::agent_provider::claude::slash_commands::ProviderSlashCommand;
use crate::agent_provider::ApprovalDecision;

/// Client half of the `initialize` handshake.
///
/// The capability block is the one from the live capture: filesystem
/// read/write plus terminal. Declaring them is not cosmetic — an agent
/// that believes the client cannot read files routes work it would
/// otherwise delegate through its own tools, so under-declaring changes
/// the agent's behaviour rather than just the handshake.
///
/// `clientInfo` is validated against a strict schema upstream: `version`
/// is REQUIRED, and omitting it fails the whole handshake with
/// `-32602 Invalid params` before a session ever starts.
pub fn initialize_params(client_name: &str) -> Value {
    json!({
        "protocolVersion": 1,
        "clientInfo": { "name": client_name, "version": env!("CARGO_PKG_VERSION") },
        "clientCapabilities": {
            "fs": { "readTextFile": true, "writeTextFile": true },
            "terminal": true
        }
    })
}

/// One entry from the `initialize` response's `authMethods`.
///
/// The list is DYNAMIC: its first entry is derived from whichever runtime
/// the launched profile is configured for, so the id changes with the
/// profile (`openai-codex` on the capture machine). Hard-coding a login
/// id — what the Cursor adapter does — authenticates the wrong runtime or
/// nothing at all the moment the user picks a different profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthMethod {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// `type` from the wire. `Some("terminal")` marks a method that opens
    /// an interactive program (`hermes --setup`) rather than one the
    /// client can complete over the protocol.
    pub method_type: Option<String>,
    /// `args` from the wire; only meaningful for a terminal method.
    pub args: Vec<String>,
}

impl AuthMethod {
    /// Whether completing this method needs a human at a terminal.
    ///
    /// Codemux has no way to host that from inside a chat pane, so such a
    /// method is never auto-selected — it is reported to the user as the
    /// command they need to run.
    pub fn is_interactive(&self) -> bool {
        self.method_type.as_deref() == Some("terminal")
    }
}

/// Read `authMethods` off an `initialize` response. An absent or
/// malformed list yields an empty vector, which the caller reads as "this
/// agent needs no authentication step" — the same answer it would give
/// for an explicit empty list.
pub fn auth_methods(response: &Value) -> Vec<AuthMethod> {
    response
        .get("authMethods")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|method| {
            let id = non_empty(method.get("id"))?;
            Some(AuthMethod {
                name: non_empty(method.get("name")).unwrap_or_else(|| id.clone()),
                id,
                description: non_empty(method.get("description")),
                method_type: non_empty(method.get("type")),
                args: method
                    .get("args")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect(),
            })
        })
        .collect()
}

/// Pick the method to send to `authenticate`, if any.
///
/// The first non-interactive entry wins, because that is the position the
/// agent puts the profile's own configured runtime in. An interactive
/// method is never chosen: `authenticate` with a terminal method id would
/// block the child on a program with no attached tty.
pub fn select_auth_method(methods: &[AuthMethod]) -> Option<&AuthMethod> {
    methods.iter().find(|method| !method.is_interactive())
}

/// The shell command that completes an interactive auth method, for the
/// error message shown when nothing else can authenticate the profile.
pub fn interactive_auth_hint(profile: &str, methods: &[AuthMethod]) -> Option<String> {
    let method = methods.iter().find(|method| method.is_interactive())?;
    let args = method.args.join(" ");
    Some(if args.is_empty() {
        format!("Run `hermes -p {profile}` to finish setting this profile up.")
    } else {
        format!("Run `hermes -p {profile} {args}` to finish setting this profile up.")
    })
}

/// `sessionId` from a `session/new` response.
pub fn session_id(response: &Value) -> Option<String> {
    non_empty(response.get("sessionId"))
}

/// One row of a `session/list` response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HermesSessionSummary {
    pub session_id: String,
    pub cwd: Option<String>,
    /// The agent's own auto-generated title for the conversation. This is
    /// the value that becomes a Codemux thread title when the thread has
    /// none of its own.
    pub title: Option<String>,
    pub updated_at: Option<String>,
}

/// Parse `session/list`. Rows without a session id are skipped rather
/// than defaulted — a row that cannot be addressed is not a session.
pub fn session_summaries(response: &Value) -> Vec<HermesSessionSummary> {
    response
        .get("sessions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|session| {
            Some(HermesSessionSummary {
                session_id: non_empty(session.get("sessionId"))?,
                cwd: non_empty(session.get("cwd")),
                title: non_empty(session.get("title")),
                updated_at: non_empty(session.get("updatedAt")),
            })
        })
        .collect()
}

/// Parse an `available_commands_update` frame into the shared
/// slash-command shape.
///
/// Hermes reports `{name, description, input?: {hint}}`. The hint is the
/// argument placeholder the composer renders, so it maps onto
/// `argument_hint`; a command with no `input` block takes no arguments.
pub fn slash_commands(update: &Value) -> Vec<ProviderSlashCommand> {
    update
        .get("availableCommands")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|command| {
            Some(ProviderSlashCommand {
                name: non_empty(command.get("name"))?,
                description: non_empty(command.get("description")).unwrap_or_default(),
                argument_hint: command
                    .pointer("/input/hint")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|hint| !hint.is_empty())
                    .map(str::to_string)
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// Context-window occupancy from a `usage_update` frame: `(used, size)`.
///
/// This frame is the ONLY source of the context meter. It carries token
/// occupancy and window size and nothing else — no input/output split, no
/// cache breakdown — so there is deliberately no cost path built on it.
pub fn context_occupancy(update: &Value) -> Option<(u64, Option<u64>)> {
    let used = update.get("used").and_then(Value::as_u64)?;
    let size = update
        .get("size")
        .and_then(Value::as_u64)
        .filter(|size| *size > 0);
    Some((used, size))
}

/// The per-turn token report on a `session/prompt` response.
///
/// Present on every turn in the capture. `total_tokens` is what the
/// context meter falls back to when a turn produced no `usage_update`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PromptUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub thought_tokens: u64,
    pub cached_read_tokens: u64,
    pub total_tokens: u64,
}

pub fn prompt_usage(response: &Value) -> Option<PromptUsage> {
    let usage = response.get("usage")?;
    let number = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    let parsed = PromptUsage {
        input_tokens: number("inputTokens"),
        output_tokens: number("outputTokens"),
        thought_tokens: number("thoughtTokens"),
        cached_read_tokens: number("cachedReadTokens"),
        total_tokens: number("totalTokens"),
    };
    (parsed != PromptUsage::default()).then_some(parsed)
}

// ---------------------------------------------------------------------------
// Permission options
// ---------------------------------------------------------------------------

/// One option offered by `session/request_permission`, normalised.
///
/// Deliberately NOT a fixed table keyed on how many options arrived. The
/// live capture shows the set changing with the profile's `approvals.mode`
/// — an edit approval offers two options, a stricter shell policy offers
/// more, and the `smart` default auto-approves shell commands without
/// asking at all. Mapping each option by its own `kind` and rendering its
/// `name` verbatim is the only shape that survives every mode without a
/// table to keep in sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionOption {
    pub option_id: String,
    /// ACP `kind`: `allow_once` / `allow_always` / `reject_once` /
    /// `reject_always`, or anything a future agent adds.
    pub kind: Option<String>,
    /// The agent's own label. Rendered verbatim — it is the only text
    /// that describes what this particular option does ("Allow edit",
    /// "Allow all edits in this session").
    pub name: String,
}

impl PermissionOption {
    /// Whether this option grants rather than refuses. Unknown kinds are
    /// neither: an option Codemux does not understand is offered to the
    /// user by name and never auto-selected.
    fn is_allow(&self) -> bool {
        matches!(self.kind.as_deref(), Some("allow_once" | "allow_always"))
    }

    fn is_reject(&self) -> bool {
        matches!(self.kind.as_deref(), Some("reject_once" | "reject_always"))
    }
}

/// Parse the `options` array of a permission request.
pub fn permission_options(params: &Value) -> Vec<PermissionOption> {
    params
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|option| {
            let option_id = non_empty(option.get("optionId"))?;
            Some(PermissionOption {
                name: non_empty(option.get("name")).unwrap_or_else(|| option_id.clone()),
                kind: non_empty(option.get("kind")),
                option_id,
            })
        })
        .collect()
}

/// The option id a decision maps onto, or `None` when the agent offered
/// nothing that expresses it.
///
/// Preference order within a decision is by `kind`, most-specific first.
/// A decision with no exact kind match falls back to any option that is at
/// least on the right side of allow/deny, so a future agent that offers
/// only `allow_always` still answers a plain Allow.
pub fn select_permission_option(
    options: &[PermissionOption],
    decision: &ApprovalDecision,
) -> Option<String> {
    let (preferred, fallback): (&[&str], fn(&PermissionOption) -> bool) = match decision {
        ApprovalDecision::AllowForSession => {
            (&["allow_always", "allow_once"], PermissionOption::is_allow)
        }
        ApprovalDecision::Allow { .. } => {
            (&["allow_once", "allow_always"], PermissionOption::is_allow)
        }
        ApprovalDecision::Deny { .. } => (
            &["reject_once", "reject_always"],
            PermissionOption::is_reject,
        ),
        ApprovalDecision::Cancel => return None,
    };
    preferred
        .iter()
        .find_map(|kind| {
            options
                .iter()
                .find(|option| option.kind.as_deref() == Some(*kind))
                .map(|option| option.option_id.clone())
        })
        .or_else(|| {
            options
                .iter()
                .find(|option| fallback(option))
                .map(|option| option.option_id.clone())
        })
}

/// The JSON-RPC result body answering a `session/request_permission`.
///
/// Shape verified live:
/// `{"outcome": {"outcome": "selected", "optionId": "<id>"}}`.
pub fn permission_response(options: &[PermissionOption], decision: &ApprovalDecision) -> Value {
    match select_permission_option(options, decision) {
        Some(option_id) => json!({
            "outcome": { "outcome": "selected", "optionId": option_id }
        }),
        None => json!({ "outcome": { "outcome": "cancelled" } }),
    }
}

// ---------------------------------------------------------------------------
// Tool calls
// ---------------------------------------------------------------------------

/// A `tool_call` / `tool_call_update` frame translated into the shape
/// Codemux's transcript renders.
#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedToolCall {
    /// Name the transcript switches on. Normalised to the vocabulary the
    /// tool-body renderers already key off (`Edit` / `MultiEdit` /
    /// `Write` / `Bash`), because those renderers ARE the diff viewer — a
    /// Hermes-native name would fall through to the generic JSON body and
    /// lose the diff entirely.
    pub tool_name: String,
    /// Input in the field names the matching renderer reads
    /// (`file_path` + `old_string`/`new_string`, or `content`, or
    /// `command`).
    pub input: Value,
    /// True when this call dispatches work to a subagent. Surfaced as a
    /// warning: upstream dispatches `delegate_task` with async delivery on
    /// and nothing drains the completion queue, so the result never comes
    /// back. Known limitation, not something the adapter can fix.
    pub is_delegation: bool,
}

/// Stable tool name for a call, when the frame carries one.
///
/// `toolCall.rawInput.tool` is the agent's own identifier (`write_file`,
/// `patch`, `terminal`) and is the only stable name available — the title
/// is prose and changes with the path it mentions. Only permission
/// requests carry `rawInput`; ordinary `session/update` frames do not,
/// which is why [`normalize_tool_call`] can also work from the content
/// shape alone.
pub fn raw_tool_name(tool_call: &Value) -> Option<String> {
    non_empty(tool_call.pointer("/rawInput/tool"))
}

/// A `{type: "diff"}` content block: what the diff viewer renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffBlock {
    pub path: String,
    /// Absent for a creation. Its PRESENCE is what distinguishes a
    /// replace from a write — verified live, and the only signal that
    /// does: both shapes carry `path` and `newText`.
    pub old_text: Option<String>,
    pub new_text: String,
}

/// Every diff block in a tool call's `content` array.
pub fn diff_blocks(tool_call: &Value) -> Vec<DiffBlock> {
    tool_call
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("diff"))
        .filter_map(|block| {
            Some(DiffBlock {
                path: non_empty(block.get("path"))?,
                old_text: block
                    .get("oldText")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                new_text: block
                    .get("newText")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .collect()
}

/// Paths named by a call's `locations` array.
///
/// Emitted alongside the diff for an edit, and on its own for a call that
/// touches a file without producing one. Kept so a tool card can still
/// name the file when there is no diff to show.
pub fn location_paths(tool_call: &Value) -> Vec<String> {
    tool_call
        .get("locations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|location| non_empty(location.get("path")))
        .collect()
}

/// Plain text carried by a call's `{type: "content"}` blocks, joined.
///
/// Shell calls report this way rather than as a diff: the initial frame
/// holds `$ <command>` and the update holds the rendered result.
pub fn content_text(tool_call: &Value) -> String {
    tool_call
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("content"))
        .filter_map(|block| block.pointer("/content/text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Tool names whose call hands work to a subagent.
///
/// An explicit list rather than a substring test, so an unrelated tool
/// that merely contains "task" is not flagged.
const DELEGATION_TOOLS: &[&str] = &["delegate_task", "delegate", "spawn_agent", "subagent"];

pub fn is_delegation_tool(tool_name: &str) -> bool {
    DELEGATION_TOOLS
        .iter()
        .any(|candidate| tool_name.eq_ignore_ascii_case(candidate))
}

/// Translate a tool call into the transcript's vocabulary.
///
/// Resolution order matters. The stable `rawInput.tool` name decides
/// delegation, because that is the one classification a content shape
/// cannot reveal. Otherwise the content shape decides, because an
/// ordinary `session/update` tool call carries no raw input at all: a
/// diff block is the strongest signal — `oldText` present means a
/// replace, absent means a creation — and `kind: "execute"` with
/// `$ `-prefixed text is a shell command.
pub fn normalize_tool_call(tool_call: &Value) -> NormalizedToolCall {
    let raw_name = raw_tool_name(tool_call);
    let kind = tool_call
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let title = tool_call
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let arguments = tool_call.pointer("/rawInput/arguments");

    if let Some(name) = raw_name.as_deref() {
        if is_delegation_tool(name) {
            return NormalizedToolCall {
                tool_name: name.to_string(),
                input: arguments.cloned().unwrap_or_else(|| json!({})),
                is_delegation: true,
            };
        }
    }

    let diffs = diff_blocks(tool_call);
    if let Some(diff) = diffs.first() {
        // A multi-hunk edit arrives as several diff blocks over the same
        // file; the renderer's MultiEdit shape shows them as one card
        // instead of N unrelated ones.
        if diffs.len() > 1 {
            return NormalizedToolCall {
                tool_name: "MultiEdit".into(),
                input: json!({
                    "file_path": diff.path,
                    "edits": diffs.iter().map(|block| json!({
                        "old_string": block.old_text.clone().unwrap_or_default(),
                        "new_string": block.new_text,
                    })).collect::<Vec<_>>(),
                }),
                is_delegation: false,
            };
        }
        return match &diff.old_text {
            Some(old_text) => NormalizedToolCall {
                tool_name: "Edit".into(),
                input: json!({
                    "file_path": diff.path,
                    "old_string": old_text,
                    "new_string": diff.new_text,
                }),
                is_delegation: false,
            },
            None => NormalizedToolCall {
                tool_name: "Write".into(),
                input: json!({ "file_path": diff.path, "content": diff.new_text }),
                is_delegation: false,
            },
        };
    }

    if kind == "execute" {
        let text = content_text(tool_call);
        let command = text
            .lines()
            .find_map(|line| line.strip_prefix("$ "))
            .map(str::to_string)
            .or_else(|| {
                title
                    .strip_prefix("terminal: ")
                    .map(str::trim)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        return NormalizedToolCall {
            tool_name: "Bash".into(),
            input: json!({ "command": command, "description": title }),
            is_delegation: false,
        };
    }

    // Anything else keeps the agent's own naming and payload. The
    // transcript renders it as a generic tool card — legible, and never a
    // reason to fail the turn.
    let locations = location_paths(tool_call);
    let mut input = arguments.cloned().unwrap_or_else(|| {
        let text = content_text(tool_call);
        if text.is_empty() {
            json!({})
        } else {
            json!({ "text": text })
        }
    });
    if let (Some(path), Some(object)) = (locations.first(), input.as_object_mut()) {
        object
            .entry("file_path")
            .or_insert_with(|| Value::String(path.clone()));
    }
    let tool_name = raw_name
        .or_else(|| (!title.is_empty()).then(|| title.to_string()))
        .or_else(|| (!kind.is_empty()).then(|| kind.to_string()))
        .unwrap_or_else(|| "tool".into());
    NormalizedToolCall {
        tool_name,
        input,
        is_delegation: false,
    }
}

/// Trimmed, non-empty string at `value`.
fn non_empty(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim from the live `initialize` response.
    fn initialize_response() -> Value {
        json!({
            "protocolVersion": 1,
            "agentInfo": {"name": "hermes-agent", "version": "0.20.6"},
            "authMethods": [
                {"id": "openai-codex", "name": "openai-codex runtime credentials",
                 "description": "Authenticate Hermes using the currently configured openai-codex runtime credentials."},
                {"id": "hermes-setup", "name": "Configure Hermes provider", "type": "terminal",
                 "args": ["--setup"],
                 "description": "Open Hermes' interactive model/provider setup in a terminal."}
            ]
        })
    }

    #[test]
    fn client_info_carries_the_version_the_agent_requires() {
        // Verified against the real agent: a `clientInfo` without
        // `version` is rejected with `-32602 Invalid params`, which fails
        // `initialize` and therefore every session start.
        let params = initialize_params("codemux");
        assert_eq!(params["clientInfo"]["name"], "codemux");
        assert_eq!(
            params["clientInfo"]["version"],
            json!(env!("CARGO_PKG_VERSION"))
        );
        assert!(params["clientInfo"]["version"]
            .as_str()
            .is_some_and(|version| !version.is_empty()));
    }

    #[test]
    fn auth_method_is_read_from_the_response_not_hard_coded() {
        let methods = auth_methods(&initialize_response());
        assert_eq!(methods.len(), 2);
        assert!(!methods[0].is_interactive());
        assert!(methods[1].is_interactive());
        assert_eq!(methods[1].args, vec!["--setup".to_string()]);
        assert_eq!(
            select_auth_method(&methods).map(|method| method.id.as_str()),
            Some("openai-codex")
        );
    }

    #[test]
    fn an_all_interactive_method_list_selects_nothing_and_yields_a_hint() {
        let methods = auth_methods(&json!({
            "authMethods": [
                {"id": "hermes-setup", "name": "Configure", "type": "terminal", "args": ["--setup"]}
            ]
        }));
        assert!(select_auth_method(&methods).is_none());
        assert_eq!(
            interactive_auth_hint("codemuxdev", &methods).as_deref(),
            Some("Run `hermes -p codemuxdev --setup` to finish setting this profile up.")
        );
    }

    #[test]
    fn permission_options_map_by_kind_and_render_their_own_names() {
        // The edit approval set, verbatim from the capture.
        let options = permission_options(&json!({
            "options": [
                {"optionId": "allow_once", "kind": "allow_once", "name": "Allow edit"},
                {"optionId": "deny", "kind": "reject_once", "name": "Deny"}
            ]
        }));
        assert_eq!(options[0].name, "Allow edit");
        assert_eq!(
            permission_response(
                &options,
                &ApprovalDecision::Allow {
                    updated_input: None,
                    updated_permissions: None
                }
            ),
            json!({"outcome": {"outcome": "selected", "optionId": "allow_once"}})
        );
        assert_eq!(
            permission_response(
                &options,
                &ApprovalDecision::Deny {
                    message: "no".into()
                }
            ),
            json!({"outcome": {"outcome": "selected", "optionId": "deny"}})
        );
        // AllowForSession has no `allow_always` here; it must still allow
        // rather than cancel the request and wedge the turn.
        assert_eq!(
            permission_response(&options, &ApprovalDecision::AllowForSession),
            json!({"outcome": {"outcome": "selected", "optionId": "allow_once"}})
        );
    }

    #[test]
    fn an_unknown_option_kind_is_offered_but_never_auto_selected() {
        let options = permission_options(&json!({
            "options": [
                {"optionId": "allow_for_path", "kind": "allow_for_path", "name": "Allow in this folder"},
                {"optionId": "deny", "kind": "reject_once", "name": "Deny"}
            ]
        }));
        // An unknown kind is not `is_allow`, so an Allow finds nothing to
        // select and the request is cancelled rather than answered with a
        // permission nobody asked for.
        assert_eq!(
            select_permission_option(
                &options,
                &ApprovalDecision::Allow {
                    updated_input: None,
                    updated_permissions: None
                }
            ),
            None
        );
        // The option still reaches the UI carrying its own label.
        assert_eq!(options[0].name, "Allow in this folder");
    }

    #[test]
    fn a_creation_becomes_write_and_a_replace_becomes_edit() {
        let write = normalize_tool_call(&json!({
            "kind": "edit",
            "title": "write: /tmp/spike_demo.txt",
            "content": [{"type": "diff", "path": "/tmp/spike_demo.txt", "newText": "hello"}]
        }));
        assert_eq!(write.tool_name, "Write");
        assert_eq!(write.input["file_path"], "/tmp/spike_demo.txt");
        assert_eq!(write.input["content"], "hello");

        let edit = normalize_tool_call(&json!({
            "kind": "edit",
            "title": "patch (replace): /tmp/spike_demo.txt",
            "content": [{
                "type": "diff", "path": "/tmp/spike_demo.txt",
                "oldText": "hello", "newText": "hello world"
            }]
        }));
        assert_eq!(edit.tool_name, "Edit");
        assert_eq!(edit.input["old_string"], "hello");
        assert_eq!(edit.input["new_string"], "hello world");
    }

    #[test]
    fn a_shell_call_becomes_bash_with_the_command_recovered() {
        let call = normalize_tool_call(&json!({
            "kind": "execute",
            "title": "terminal: echo spike-ok",
            "locations": [],
            "content": [{"type": "content", "content": {"type": "text", "text": "$ echo spike-ok"}}]
        }));
        assert_eq!(call.tool_name, "Bash");
        assert_eq!(call.input["command"], "echo spike-ok");
    }

    #[test]
    fn a_delegation_call_is_flagged_from_its_stable_tool_name() {
        let call = normalize_tool_call(&json!({
            "kind": "other",
            "title": "delegate: review the diff",
            "rawInput": {"tool": "delegate_task", "arguments": {"prompt": "review"}}
        }));
        assert!(call.is_delegation);
        assert_eq!(call.input["prompt"], "review");
    }

    #[test]
    fn an_unrecognised_call_keeps_its_own_name_and_gains_its_location() {
        let call = normalize_tool_call(&json!({
            "kind": "read",
            "title": "read: /etc/hosts",
            "locations": [{"path": "/etc/hosts"}],
            "content": [{"type": "content", "content": {"type": "text", "text": "127.0.0.1"}}]
        }));
        assert_eq!(call.tool_name, "read: /etc/hosts");
        assert_eq!(call.input["file_path"], "/etc/hosts");
        assert!(!call.is_delegation);
    }

    #[test]
    fn usage_and_command_frames_parse_from_the_captured_shapes() {
        assert_eq!(
            context_occupancy(
                &json!({"sessionUpdate": "usage_update", "size": 272000, "used": 8398})
            ),
            Some((8398, Some(272_000)))
        );
        let commands = slash_commands(&json!({
            "availableCommands": [
                {"name": "help", "description": "List available commands"},
                {"name": "model", "description": "Show current model",
                 "input": {"hint": "model name to switch to"}}
            ]
        }));
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].argument_hint, "");
        assert_eq!(commands[1].argument_hint, "model name to switch to");
    }

    #[test]
    fn session_list_carries_the_agent_title() {
        let sessions = session_summaries(&json!({
            "sessions": [{
                "sessionId": "ebd7fd38", "cwd": "/tmp",
                "title": "Create /tmp/spike_demo.txt", "updatedAt": "2026-08-28T13:40:28.751Z"
            }, {
                "cwd": "/tmp"
            }]
        }));
        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0].title.as_deref(),
            Some("Create /tmp/spike_demo.txt")
        );
    }

    #[test]
    fn prompt_usage_reads_the_captured_turn_report() {
        let usage = prompt_usage(&json!({
            "stopReason": "end_turn",
            "usage": {"inputTokens": 47491, "outputTokens": 148, "thoughtTokens": 47,
                      "cachedReadTokens": 33280, "totalTokens": 47639}
        }))
        .expect("captured response carries usage");
        assert_eq!(usage.total_tokens, 47639);
        assert_eq!(usage.thought_tokens, 47);
        assert!(prompt_usage(&json!({"stopReason": "end_turn"})).is_none());
    }
}
