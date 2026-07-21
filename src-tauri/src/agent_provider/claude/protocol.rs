//! Wire-level types for the JSON-RPC the claude-agent sidecar speaks.
//!
//! Every struct that crosses the wire uses
//! `#[serde(rename_all = "camelCase")]` because the sidecar's TS code
//! uses camelCase keys.
//!
//! This module ONLY models our framing — the SDK messages that ride
//! inside `sdk-message` notifications are kept as opaque
//! `serde_json::Value` so the classifier in [`super::translate`] can
//! inspect them structurally without a tight schema dependency.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent_provider::ApprovalDecision;

// ---------------------------------------------------------------------------
// Method names — kept as constants so a typo fails at compile time.
// ---------------------------------------------------------------------------

pub const METHOD_PING: &str = "ping";
pub const METHOD_START_SESSION: &str = "start-session";
pub const METHOD_SEND_TURN: &str = "send-turn";
pub const METHOD_INTERRUPT: &str = "interrupt";
pub const METHOD_SET_MODEL: &str = "set-model";
pub const METHOD_SET_PERMISSION_MODE: &str = "set-permission-mode";
pub const METHOD_RESPOND_TO_REQUEST: &str = "respond-to-request";
pub const METHOD_RESPOND_TO_USER_INPUT: &str = "respond-to-user-input";
pub const METHOD_INITIALIZATION_RESULT: &str = "initialization-result";
pub const METHOD_STOP_SESSION: &str = "stop-session";
pub const METHOD_PROBE_INSTALLED: &str = "probe-installed";
pub const METHOD_PROBE_AUTHENTICATED: &str = "probe-authenticated";

// ---------------------------------------------------------------------------
// Request params — client → sidecar
// ---------------------------------------------------------------------------

/// Params for the sidecar's `start-session` RPC.
///
/// Every optional field is skipped on serialization when `None` so
/// the wire payload stays small and the sidecar's strict field
/// validation does not see spurious nulls.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartSessionParams {
    pub thread_id: String,
    pub cwd: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fast_mode: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_dangerously_skip_permissions: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additional_directories: Option<Vec<PathBuf>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub path_to_claude_code_executable: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_args: Option<serde_json::Map<String, Value>>,
    /// Stage 3 — tools to register with the SDK as the in-process
    /// `codemux` virtual MCP server. Each entry is one tool from the
    /// MCP runtime registry, with its prefixed name. Empty when the
    /// runtime has nothing to expose, or when the provider was
    /// constructed without an `mcp_registry`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_tools: Vec<McpToolEntry>,
}

/// Method name for Stage 4 dynamic tool refresh. The Claude session
/// listens to `McpRegistry::subscribe_status()`; on every transition
/// it re-collects tools and pushes them via this RPC. Idempotent —
/// the sidecar calls `query.setMcpServers(...)` with the latest
/// snapshot.
pub const METHOD_UPDATE_MCP_TOOLS: &str = "update-mcp-tools";

/// Wire shape of `update-mcp-tools` request params.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMcpToolsParams {
    pub thread_id: String,
    pub mcp_tools: Vec<McpToolEntry>,
}

/// Wire shape of a single MCP tool registration. Mirrored on the
/// sidecar side as `RegisteredMcpTool` (`mcp-bridge.ts`). The
/// `prefixed_name` is what the agent sees and what permission rules
/// key on; `input_schema` is opaque JSON Schema we forward verbatim.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolEntry {
    pub name: String,
    pub prefixed_name: String,
    pub description: Option<String>,
    pub input_schema: Value,
    pub server_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendTurnParams {
    pub thread_id: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_override: Option<String>,
    /// Inline image attachments. Empty list serializes verbatim so the
    /// sidecar's strict shape validator can branch on length without
    /// special-casing missing keys.
    #[serde(default)]
    pub images: Vec<SendTurnImage>,
}

/// Wire-shape of a single image attachment. Bytes are base64-encoded
/// here (rather than at the SDK call site) so the JSON-RPC frame stays
/// strictly text and the sidecar can pipe the data straight into the
/// Anthropic SDK's `content` blocks without re-decoding.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendTurnImage {
    /// Media type — "image/png", "image/jpeg", "image/webp", "image/gif".
    pub media_type: String,
    /// Standard (non-URL-safe) base64 of the raw bytes.
    pub data_base64: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InterruptParams {
    pub thread_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetModelParams {
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetPermissionModeParams {
    pub thread_id: String,
    pub mode: String,
}

/// Sidecar wire form of an approval decision. Mirrors what the
/// sidecar's `permissions.ts` expects: either an `allow` (optional
/// `updatedInput` / `updatedPermissions`) or a `deny` (required
/// `message`, optional `interrupt`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarDecision {
    pub behavior: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_input: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_permissions: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interrupt: Option<bool>,
}

impl From<ApprovalDecision> for SidecarDecision {
    fn from(value: ApprovalDecision) -> Self {
        match value {
            ApprovalDecision::Allow {
                updated_input,
                updated_permissions,
            } => Self {
                behavior: "allow".into(),
                updated_input,
                updated_permissions: updated_permissions.map(Value::Array),
                message: None,
                interrupt: None,
            },
            ApprovalDecision::AllowForSession => Self {
                behavior: "allow".into(),
                updated_input: None,
                updated_permissions: Some(Value::Array(vec![])),
                message: None,
                interrupt: None,
            },
            ApprovalDecision::Deny { message } => Self {
                behavior: "deny".into(),
                updated_input: None,
                updated_permissions: None,
                message: Some(message),
                interrupt: None,
            },
            ApprovalDecision::Cancel => Self {
                behavior: "deny".into(),
                updated_input: None,
                updated_permissions: None,
                message: Some("User cancelled tool execution.".into()),
                interrupt: Some(true),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RespondToRequestParams {
    pub thread_id: String,
    pub request_id: String,
    pub decision: SidecarDecision,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RespondToUserInputParams {
    pub thread_id: String,
    pub request_id: String,
    pub answers: Vec<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StopSessionParams {
    pub thread_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeInstalledParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeAuthenticatedParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary_path: Option<String>,
}

// ---------------------------------------------------------------------------
// Response shapes — sidecar → client
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartSessionResponse {
    pub thread_id: String,
    pub path_to_claude_code_executable: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendTurnResponse {
    #[serde(default)]
    pub turn_started: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StopSessionResponse {
    #[serde(default)]
    pub already_closed: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeInstalledResponse {
    #[serde(default)]
    pub installed: bool,
    #[serde(default)]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeAuthenticatedResponse {
    /// `"authenticated"` | `"unauthenticated"` | `"unknown"`.
    pub status: String,
    #[serde(default)]
    pub message: Option<String>,
}

// ---------------------------------------------------------------------------
// Notifications — sidecar → client
// ---------------------------------------------------------------------------

/// Structured error payload carried on `session-error` notifications.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SidecarError {
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub stack: Option<String>,
}

/// Every notification the sidecar can push us, discriminated by
/// method. Unknown methods fall through to [`SidecarNotification::Unknown`]
/// so the translator can emit a
/// [`ProviderRuntimeEvent::RuntimeWarning`](crate::agent_provider::ProviderRuntimeEvent::RuntimeWarning)
/// rather than dropping the event.
///
/// NOTE: we do NOT use `#[serde(tag, content)]` here because the
/// `Unknown` variant must accept arbitrary shapes. Instead, the
/// caller deserializes method+params separately (from
/// `json_rpc_child::Notification`) and dispatches through
/// [`SidecarNotification::from_method_params`].
#[derive(Debug, Clone)]
pub enum SidecarNotification {
    SessionConfigured {
        thread_id: String,
        path_to_claude_code_executable: PathBuf,
    },
    SdkMessage {
        thread_id: String,
        message: Value,
    },
    RequestOpened {
        thread_id: String,
        request_id: String,
        tool_name: String,
        tool_input: Value,
        /// Claude's own tool-use identifier, forwarded so the runtime can
        /// merge a permission request with its originating tool_use row.
        /// `None` when the sidecar did not populate `toolUseId`.
        tool_use_id: Option<String>,
        kind: String,
    },
    RequestResolved {
        thread_id: String,
        request_id: String,
        decision: Value,
    },
    UserInputRequested {
        thread_id: String,
        tool_use_id: Option<String>,
        input: Value,
    },
    PlanProposed {
        thread_id: String,
        tool_use_id: Option<String>,
        plan: Value,
    },
    SessionEnded {
        thread_id: String,
        reason: String,
    },
    /// The sidecar ended the SDK query's active turn in response to an
    /// explicit `interrupt` RPC. Unlike `session-ended {reason:
    /// "interrupted"}` (a spontaneous abort that tears the session down),
    /// this notification means the session survives — the next send-turn
    /// transparently rebuilds a resumed query.
    TurnInterrupted {
        thread_id: String,
    },
    SessionError {
        thread_id: String,
        error: SidecarError,
    },
    /// Sidecar observed the SDK's internal session_id on an incoming
    /// message. Stable for the life of the session; the adapter
    /// stashes it as the resume cursor.
    SdkSessionId {
        thread_id: String,
        session_id: String,
    },
    /// The sidecar could not resume the persisted SDK session (its
    /// on-disk conversation JSONL was gone), so it transparently rebuilt
    /// a FRESH query with no `resume` and replayed the user's turn. The
    /// stale cursor must be cleared: `state.sdk_session_id` is reset and
    /// the persisted id is dropped. The rebuilt query emits a new
    /// `sdk-session-id` shortly after, which re-populates both. The
    /// replayed turn stays `Running` until its real `result` lands.
    ResumeFallback {
        thread_id: String,
        stale_session_id: Option<String>,
    },
    Unknown {
        method: String,
        params: Value,
    },
}

impl SidecarNotification {
    /// Best-effort classify-and-decode. Never fails — unknown or
    /// malformed payloads land in
    /// [`SidecarNotification::Unknown`] so the translator can surface
    /// them as a `RuntimeWarning`.
    pub fn from_method_params(method: &str, params: Value) -> Self {
        fn field_string(v: &Value, name: &str) -> String {
            v.get(name)
                .and_then(|x| x.as_str())
                .map(String::from)
                .unwrap_or_default()
        }
        fn field_opt_string(v: &Value, name: &str) -> Option<String> {
            v.get(name)
                .and_then(|x| x.as_str())
                .map(String::from)
        }
        fn field_value(v: &Value, name: &str) -> Value {
            v.get(name).cloned().unwrap_or(Value::Null)
        }
        match method {
            "session-configured" => Self::SessionConfigured {
                thread_id: field_string(&params, "threadId"),
                path_to_claude_code_executable: field_string(
                    &params,
                    "pathToClaudeCodeExecutable",
                )
                .into(),
            },
            "sdk-message" => Self::SdkMessage {
                thread_id: field_string(&params, "threadId"),
                message: field_value(&params, "message"),
            },
            "request-opened" => Self::RequestOpened {
                thread_id: field_string(&params, "threadId"),
                request_id: field_string(&params, "requestId"),
                tool_name: field_string(&params, "toolName"),
                tool_input: field_value(&params, "toolInput"),
                tool_use_id: field_opt_string(&params, "toolUseId"),
                kind: field_string(&params, "kind"),
            },
            "request-resolved" => Self::RequestResolved {
                thread_id: field_string(&params, "threadId"),
                request_id: field_string(&params, "requestId"),
                decision: field_value(&params, "decision"),
            },
            "user-input-requested" => Self::UserInputRequested {
                thread_id: field_string(&params, "threadId"),
                tool_use_id: field_opt_string(&params, "toolUseId"),
                input: field_value(&params, "input"),
            },
            "plan-proposed" => Self::PlanProposed {
                thread_id: field_string(&params, "threadId"),
                tool_use_id: field_opt_string(&params, "toolUseId"),
                plan: field_value(&params, "plan"),
            },
            "session-ended" => Self::SessionEnded {
                thread_id: field_string(&params, "threadId"),
                reason: field_string(&params, "reason"),
            },
            "turn-interrupted" => Self::TurnInterrupted {
                thread_id: field_string(&params, "threadId"),
            },
            "session-error" => Self::SessionError {
                thread_id: field_string(&params, "threadId"),
                error: serde_json::from_value(field_value(&params, "error"))
                    .unwrap_or_default(),
            },
            "sdk-session-id" => Self::SdkSessionId {
                thread_id: field_string(&params, "threadId"),
                session_id: field_string(&params, "sessionId"),
            },
            "resume-fallback" => Self::ResumeFallback {
                thread_id: field_string(&params, "threadId"),
                stale_session_id: field_opt_string(&params, "staleSessionId"),
            },
            _ => Self::Unknown {
                method: method.to_string(),
                params,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ─── Stage 6 — image attachments ───
    // The wire contract: each image is a `{mediaType, dataBase64}`
    // pair on the `images` array, alongside the existing text. The
    // sidecar TS code converts these to Anthropic SDK `image/base64`
    // content blocks; the Rust adapter is responsible for the
    // base64 encoding so the JSON-RPC frame stays text-only.

    #[test]
    fn send_turn_serializes_empty_images_array_when_no_images() {
        let p = SendTurnParams {
            thread_id: "t1".into(),
            text: "hi".into(),
            model_override: None,
            images: vec![],
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["threadId"], "t1");
        assert_eq!(v["text"], "hi");
        // Empty list still serializes as a key — the sidecar's
        // strict shape validator branches on length, not presence.
        assert_eq!(v["images"], json!([]));
    }

    #[test]
    fn send_turn_serializes_image_block_in_camel_case() {
        let p = SendTurnParams {
            thread_id: "t1".into(),
            text: "describe this".into(),
            model_override: None,
            images: vec![
                SendTurnImage {
                    media_type: "image/png".into(),
                    data_base64: "iVBORw0KGgo=".into(),
                },
                SendTurnImage {
                    media_type: "image/jpeg".into(),
                    data_base64: "/9j/4AA=".into(),
                },
            ],
        };
        let v = serde_json::to_value(&p).unwrap();
        let imgs = v["images"].as_array().expect("images must be an array");
        assert_eq!(imgs.len(), 2);
        assert_eq!(imgs[0]["mediaType"], "image/png");
        assert_eq!(imgs[0]["dataBase64"], "iVBORw0KGgo=");
        assert_eq!(imgs[1]["mediaType"], "image/jpeg");
        assert_eq!(imgs[1]["dataBase64"], "/9j/4AA=");
    }

    #[test]
    fn start_session_serializes_camel_case_and_skips_none() {
        let p = StartSessionParams {
            thread_id: "t1".into(),
            cwd: "/tmp".into(),
            model: Some("claude-opus-4-7".into()),
            effort: None,
            fast_mode: Some(true),
            permission_mode: None,
            allow_dangerously_skip_permissions: None,
            additional_directories: None,
            settings: None,
            resume: None,
            session_id: None,
            path_to_claude_code_executable: "/usr/bin/claude".into(),
            extra_args: None,
            mcp_tools: Vec::new(),
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["threadId"], "t1");
        assert_eq!(v["pathToClaudeCodeExecutable"], "/usr/bin/claude");
        assert_eq!(v["model"], "claude-opus-4-7");
        assert!(v.get("effort").is_none());
        assert_eq!(v["fastMode"], true);
        assert!(v.get("permissionMode").is_none());
    }

    #[test]
    fn decision_allow_passes_updated_input_through() {
        let d: SidecarDecision = ApprovalDecision::Allow {
            updated_input: Some(json!({"edited": true})),
            updated_permissions: None,
        }
        .into();
        assert_eq!(d.behavior, "allow");
        assert_eq!(d.updated_input, Some(json!({"edited": true})));
        assert!(d.message.is_none());
        assert!(d.updated_permissions.is_none());
    }

    #[test]
    fn decision_allow_forwards_updated_permissions_as_array() {
        // "Allow always" flows through `updated_permissions` — an opaque
        // `PermissionUpdate[]` per the SDK — wrapped back into a JSON
        // array on the wire.
        let rule = json!({
            "type": "addRules",
            "rules": [{"toolName": "Bash"}],
            "behavior": "allow",
            "destination": "localSettings"
        });
        let d: SidecarDecision = ApprovalDecision::Allow {
            updated_input: None,
            updated_permissions: Some(vec![rule.clone()]),
        }
        .into();
        assert_eq!(d.behavior, "allow");
        assert_eq!(d.updated_permissions, Some(json!([rule])));
    }

    #[test]
    fn decision_allow_for_session_emits_empty_updated_permissions() {
        let d: SidecarDecision = ApprovalDecision::AllowForSession.into();
        assert_eq!(d.behavior, "allow");
        assert_eq!(d.updated_permissions, Some(json!([])));
    }

    #[test]
    fn decision_deny_carries_message() {
        let d: SidecarDecision = ApprovalDecision::Deny {
            message: "nope".into(),
        }
        .into();
        assert_eq!(d.behavior, "deny");
        assert_eq!(d.message.as_deref(), Some("nope"));
    }

    #[test]
    fn decision_cancel_sets_interrupt() {
        let d: SidecarDecision = ApprovalDecision::Cancel.into();
        assert_eq!(d.behavior, "deny");
        assert_eq!(d.interrupt, Some(true));
    }

    // ─────────────────────────────────────────────────────────────
    // Stage 5 wire contract — the inner `SidecarDecision` tests
    // above prove the From<> impl forwards the field, but they do
    // NOT prove the field survives JSON serialization of the
    // wrapping `RespondToRequestParams` (which is what actually
    // hits the sidecar's stdin). These tests lock the outer shape.
    // ─────────────────────────────────────────────────────────────

    #[test]
    fn respond_to_request_params_serialize_with_nested_updated_permissions() {
        let rule = json!({
            "type": "addRules",
            "rules": [{"toolName": "Bash"}],
            "behavior": "allow",
            "destination": "localSettings",
        });
        let params = RespondToRequestParams {
            thread_id: "t-1".into(),
            request_id: "r-1".into(),
            decision: ApprovalDecision::Allow {
                updated_input: None,
                updated_permissions: Some(vec![rule.clone()]),
            }
            .into(),
        };

        let v = serde_json::to_value(&params).expect("serialize");

        // Outer keys are camelCase per `#[serde(rename_all = ...)]`
        // on the struct — confirms the wrapping survives.
        assert_eq!(v["threadId"], json!("t-1"));
        assert_eq!(v["requestId"], json!("r-1"));
        // Nested decision carries the array intact, camelCased.
        assert_eq!(v["decision"]["behavior"], json!("allow"));
        assert_eq!(v["decision"]["updatedPermissions"], json!([rule]));
        // `updatedInput` and `message` are absent thanks to
        // `skip_serializing_if = "Option::is_none"` — important
        // because the SDK distinguishes "not provided" from `null`
        // for `updatedInput` (falls back to the original toolInput).
        assert!(v["decision"].get("updatedInput").is_none());
        assert!(v["decision"].get("message").is_none());
        assert!(v["decision"].get("interrupt").is_none());
    }

    #[test]
    fn respond_to_request_params_omit_updated_permissions_for_one_shot_allow() {
        // Stage 5 "Allow once" → `updated_permissions: None`. The
        // wrapping struct must NOT emit `updatedPermissions: null`
        // either; the field is just absent.
        let params = RespondToRequestParams {
            thread_id: "t-1".into(),
            request_id: "r-1".into(),
            decision: ApprovalDecision::Allow {
                updated_input: None,
                updated_permissions: None,
            }
            .into(),
        };

        let v = serde_json::to_value(&params).expect("serialize");

        assert_eq!(v["decision"]["behavior"], json!("allow"));
        assert!(v["decision"].get("updatedPermissions").is_none());
    }

    #[test]
    fn respond_to_request_params_emit_empty_array_for_allow_for_session() {
        // The legacy `AllowForSession` variant emits an explicit
        // empty `updatedPermissions: []` (per the existing inner
        // test at `decision_allow_for_session_emits_empty_updated_permissions`).
        // Lock that the empty-array form survives the wrapping
        // struct too — the SDK distinguishes `[]` from omission.
        let params = RespondToRequestParams {
            thread_id: "t-1".into(),
            request_id: "r-1".into(),
            decision: ApprovalDecision::AllowForSession.into(),
        };

        let v = serde_json::to_value(&params).expect("serialize");

        assert_eq!(v["decision"]["updatedPermissions"], json!([]));
    }

    #[test]
    fn notification_from_method_classifies_known_events() {
        let n = SidecarNotification::from_method_params(
            "session-configured",
            json!({"threadId": "t", "pathToClaudeCodeExecutable": "/usr/bin/claude"}),
        );
        match n {
            SidecarNotification::SessionConfigured { thread_id, .. } => {
                assert_eq!(thread_id, "t")
            }
            _ => panic!("expected SessionConfigured"),
        }

        let n = SidecarNotification::from_method_params(
            "sdk-message",
            json!({"threadId": "t", "message": {"type": "assistant"}}),
        );
        match n {
            SidecarNotification::SdkMessage { thread_id, message } => {
                assert_eq!(thread_id, "t");
                assert_eq!(message["type"], "assistant");
            }
            _ => panic!("expected SdkMessage"),
        }

        let n = SidecarNotification::from_method_params(
            "request-opened",
            json!({
                "threadId": "t", "requestId": "r",
                "toolName": "Bash", "toolInput": {"cmd": "ls"},
                "kind": "command"
            }),
        );
        match n {
            SidecarNotification::RequestOpened { kind, request_id, .. } => {
                assert_eq!(kind, "command");
                assert_eq!(request_id, "r");
            }
            _ => panic!("expected RequestOpened"),
        }

        let n = SidecarNotification::from_method_params(
            "session-ended",
            json!({"threadId": "t", "reason": "iteration-complete"}),
        );
        match n {
            SidecarNotification::SessionEnded { reason, .. } => {
                assert_eq!(reason, "iteration-complete")
            }
            _ => panic!("expected SessionEnded"),
        }
    }

    #[test]
    fn notification_from_method_classifies_resume_fallback() {
        let n = SidecarNotification::from_method_params(
            "resume-fallback",
            json!({"threadId": "t", "staleSessionId": "uuid-stale"}),
        );
        match n {
            SidecarNotification::ResumeFallback {
                thread_id,
                stale_session_id,
            } => {
                assert_eq!(thread_id, "t");
                assert_eq!(stale_session_id.as_deref(), Some("uuid-stale"));
            }
            _ => panic!("expected ResumeFallback"),
        }
    }

    #[test]
    fn notification_resume_fallback_allows_null_stale_session_id() {
        // The sidecar may not know the stale id (e.g. it was never
        // observed). `staleSessionId` absent / null → `None`.
        let n = SidecarNotification::from_method_params(
            "resume-fallback",
            json!({"threadId": "t"}),
        );
        match n {
            SidecarNotification::ResumeFallback {
                thread_id,
                stale_session_id,
            } => {
                assert_eq!(thread_id, "t");
                assert_eq!(stale_session_id, None);
            }
            _ => panic!("expected ResumeFallback"),
        }
    }

    #[test]
    fn notification_unknown_method_preserves_raw_params() {
        let n = SidecarNotification::from_method_params(
            "never-heard-of-it",
            json!({"some": "payload"}),
        );
        match n {
            SidecarNotification::Unknown { method, params } => {
                assert_eq!(method, "never-heard-of-it");
                assert_eq!(params["some"], "payload");
            }
            _ => panic!("expected Unknown"),
        }
    }
}
