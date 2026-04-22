//! Wire-level JSON-RPC types spoken between Codemux and the `codex
//! app-server` subprocess.
//!
//! # Convention
//!
//! The Codex app-server uses `camelCase` on the wire (matching the
//! JavaScript/TypeScript host that originally drives it). These Rust
//! structs mirror that convention via `#[serde(rename_all = "camelCase")]`
//! unless a struct is purely internal-to-this-module. Every public
//! request/response/notification type carries a doc comment linking it to
//! the method it describes.
//!
//! # Shape
//!
//! * [`InitializeParams`], [`ThreadStartParams`], [`ThreadResumeParams`],
//!   [`TurnStartParams`], [`TurnInterruptParams`], [`ThreadReadParams`],
//!   [`ThreadRollbackParams`] — client → server requests.
//! * [`ThreadStartResponse`], [`TurnStartResponse`],
//!   [`AccountReadResponse`] — response payloads we care about.
//! * [`NotificationMessage`] — server → client notifications,
//!   discriminated on the JSON-RPC `method` field.
//! * [`ServerRequestMessage`] — server → client requests requiring a
//!   response, discriminated on method.
//! * [`ApprovalResponse`] — the payload we send back to resolve any of the
//!   four `*requestApproval` / `requestUserInput` methods.
//!
//! # Fallback variants
//!
//! Both [`NotificationMessage`] and [`ServerRequestMessage`] include an
//! `Unknown` variant so the adapter never drops an upstream message — the
//! translator promotes unknowns to
//! [`ProviderRuntimeEvent::RuntimeWarning`](crate::agent_provider::ProviderRuntimeEvent::RuntimeWarning).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent_provider::ApprovalDecision;

/// Substrings that identify a "thread not known" error from a failed
/// `thread/resume` JSON-RPC call, triggering an automatic fallback to
/// `thread/start`.
///
/// Inferred list. The upstream reference that prompted this adapter
/// mentions a `RECOVERABLE_THREAD_RESUME_ERROR_SNIPPETS` constant but the
/// exact string set was not cited, so these are the reasonable
/// candidates. Matching is case-insensitive against whatever error
/// message the RPC call surfaces.
///
/// An incomplete list is safe: a resume error that does not match falls
/// through to a plain [`ProviderError::RpcError`](crate::agent_provider::ProviderError::RpcError)
/// instead of being auto-recovered. The session ends up broken, but
/// nothing misbehaves silently.
///
// TODO: verify against real codex app-server error messages once the
// adapter is exercised in real failure scenarios.
pub const RECOVERABLE_THREAD_RESUME_ERROR_SNIPPETS: &[&str] = &[
    "thread not found",
    "unknown thread",
    "no such thread",
    "thread does not exist",
];

// ---------------------------------------------------------------------------
// initialize
// ---------------------------------------------------------------------------

/// Parameters for the `initialize` JSON-RPC method.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    /// Client identification, shown in Codex server-side logs / tracing.
    pub client_info: ClientInfo,
    /// Feature toggles the client is asserting support for.
    pub capabilities: Capabilities,
}

/// Human-meaningful client identification block sent during `initialize`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    /// Machine-parsable identifier.
    pub name: String,
    /// Human-readable product name.
    pub title: String,
    /// Client semver.
    pub version: String,
}

/// Capability flags sent during `initialize`.
///
/// Codex currently only cares about the `experimentalApi` flag; we mirror
/// the `camelCase` wire shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    /// Opts into the experimental API surface Codex exposes to upstream
    /// clients. Mirrors the upstream default.
    pub experimental_api: bool,
}

// ---------------------------------------------------------------------------
// thread/start and thread/resume
// ---------------------------------------------------------------------------

/// Parameters for the `thread/start` JSON-RPC method.
///
/// All fields except `experimental_raw_events` are optional and reflect
/// user-chosen session overrides.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartParams {
    /// Model identifier to bind to this session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Service tier override (e.g. `"business"`). Provider-specific.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    /// Working directory for the new thread.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// Collaboration-mode string (maps roughly to runtime modes like
    /// `"auto-accept"` / `"interactive"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collaboration_mode: Option<String>,
    /// Approval policy — one of `"untrusted" | "on-request" | "never"`.
    /// Paired with `sandbox` via the Codex runtime-mode table.
    #[serde(skip_serializing_if = "Option::is_none", rename = "approvalPolicy")]
    pub approval_policy: Option<String>,
    /// Sandbox mode — one of `"read-only" | "workspace-write" |
    /// "danger-full-access"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
    /// Whether to subscribe to raw experimental events. We always send
    /// `false` — the canonical event schema is synthesised from the
    /// documented notifications.
    pub experimental_raw_events: bool,
}

/// Parameters for the `thread/resume` JSON-RPC method. Shares most fields
/// with [`ThreadStartParams`] but adds the `thread_id` to resume.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadResumeParams {
    /// Codex's opaque thread identifier, retrieved from the resume cursor.
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collaboration_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "approvalPolicy")]
    pub approval_policy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
    pub experimental_raw_events: bool,
}

/// Response to `thread/start` / `thread/resume`.
///
/// Codex has historically returned either `{ "thread": { "id": "..." } }`
/// OR `{ "threadId": "..." }`. We accept both and expose a single
/// [`ThreadStartResponse::thread_id`] accessor.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ThreadStartResponse {
    /// Newer shape: nested `thread` object.
    Nested {
        /// Nested thread info.
        thread: ThreadInfo,
    },
    /// Legacy shape: flat `threadId` string.
    Flat {
        /// Thread identifier.
        #[serde(rename = "threadId")]
        thread_id: String,
    },
}

impl ThreadStartResponse {
    /// Extract the Codex-side thread identifier regardless of response
    /// shape.
    pub fn thread_id(&self) -> &str {
        match self {
            Self::Nested { thread } => &thread.id,
            Self::Flat { thread_id } => thread_id,
        }
    }
}

/// Nested thread info inside [`ThreadStartResponse::Nested`].
#[derive(Debug, Clone, Deserialize)]
pub struct ThreadInfo {
    /// Codex thread identifier.
    pub id: String,
}

// ---------------------------------------------------------------------------
// turn/start and turn/interrupt
// ---------------------------------------------------------------------------

/// Parameters for the `turn/start` JSON-RPC method.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartParams {
    /// Codex thread identifier returned by `thread/start`.
    pub thread_id: String,
    /// Ordered list of input items (text and optional images).
    pub input: Vec<TurnInputItem>,
    /// Per-turn model override. Falls through to the session default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Service tier override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    /// Reasoning-effort hint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Per-turn collaboration-mode override.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collaboration_mode: Option<String>,
}

/// A single item in [`TurnStartParams::input`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TurnInputItem {
    /// Plain-text user turn fragment.
    Text {
        /// Rendered text content.
        text: String,
        /// Reserved for future rich-text elements; always emitted as an
        /// empty array for now to mirror upstream behaviour.
        text_elements: Vec<Value>,
    },
    /// An image attachment. The URL scheme is provider-specific (Codex
    /// typically accepts `data:` URIs for local bytes).
    Image {
        /// Fully-resolved URL pointing at the image.
        url: String,
    },
}

/// Response to `turn/start`.
#[derive(Debug, Clone, Deserialize)]
pub struct TurnStartResponse {
    /// Information about the newly started turn.
    pub turn: TurnInfo,
}

/// Minimal turn info payload.
#[derive(Debug, Clone, Deserialize)]
pub struct TurnInfo {
    /// Turn identifier assigned by Codex.
    pub id: String,
}

/// Parameters for `turn/interrupt`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnInterruptParams {
    /// Codex thread identifier.
    pub thread_id: String,
    /// Turn identifier to interrupt.
    pub turn_id: String,
}

// ---------------------------------------------------------------------------
// thread/read and thread/rollback
// ---------------------------------------------------------------------------

/// Parameters for `thread/read`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadReadParams {
    /// Codex thread identifier to read.
    pub thread_id: String,
    /// Whether the response should include individual turn payloads.
    pub include_turns: bool,
}

/// Parameters for `thread/rollback`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadRollbackParams {
    /// Codex thread identifier to roll back.
    pub thread_id: String,
    /// How many turns to strip off the tail.
    pub num_turns: u32,
}

// ---------------------------------------------------------------------------
// account/read
// ---------------------------------------------------------------------------

/// Response to `account/read`.
///
/// Mirrors the subset the upstream reference actually consumes.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountReadResponse {
    /// Account type tag (e.g. `"personal"`, `"enterprise"`).
    #[serde(rename = "type")]
    pub account_type: String,
    /// Optional subscription plan name.
    pub plan_type: Option<String>,
    /// Whether spark mode is enabled for this account.
    pub spark_enabled: Option<bool>,
}

// ---------------------------------------------------------------------------
// Notifications (server → client, no id)
// ---------------------------------------------------------------------------

/// A server-pushed notification. Dispatched on the JSON-RPC `method`
/// string.
///
/// Variants carry the decoded `params` payload. Anything with a method we
/// do not recognise lands in [`NotificationMessage::Unknown`] with the raw
/// JSON preserved so the adapter can emit a
/// [`ProviderRuntimeEvent::RuntimeWarning`](crate::agent_provider::ProviderRuntimeEvent::RuntimeWarning).
#[derive(Debug, Clone)]
pub enum NotificationMessage {
    /// `thread/started` — Codex acknowledges the thread exists and emits
    /// its server-side identifier.
    ThreadStarted(ThreadStartedParams),
    /// `turn/started` — a new turn has begun.
    TurnStarted(TurnStartedParams),
    /// `turn/completed` — a turn finished, successfully or otherwise.
    TurnCompleted(TurnCompletedParams),
    /// `error` — transient / persistent error with optional retry hint.
    Error(ErrorParams),
    /// `item/agentMessage/delta` — streaming assistant text fragment.
    AgentMessageDelta(AgentMessageDeltaParams),
    /// `item/agentMessage` — finalised assistant message.
    AgentMessage(AgentMessageParams),
    /// `item/toolCall*` — the family of tool-lifecycle notifications.
    /// Carried as raw JSON because the exact shape of each subtype is
    /// provider-internal and evolves more quickly than this adapter.
    ToolCall {
        /// Original method string (e.g. `"item/toolCall/started"`).
        method: String,
        /// Raw payload.
        params: Value,
    },
    /// Any other method is surfaced as an unknown fallback so the adapter
    /// can emit a runtime warning.
    Unknown {
        /// Original method string.
        method: String,
        /// Raw payload.
        params: Value,
    },
}

impl NotificationMessage {
    /// Build a typed notification from the raw method + params pair. Never
    /// returns an error — unrecognised or malformed messages fall through
    /// to [`NotificationMessage::Unknown`].
    pub fn from_raw(method: &str, params: Value) -> Self {
        match method {
            "thread/started" => match serde_json::from_value(params.clone()) {
                Ok(p) => Self::ThreadStarted(p),
                Err(_) => Self::Unknown {
                    method: method.to_string(),
                    params,
                },
            },
            "turn/started" => match serde_json::from_value(params.clone()) {
                Ok(p) => Self::TurnStarted(p),
                Err(_) => Self::Unknown {
                    method: method.to_string(),
                    params,
                },
            },
            "turn/completed" => match serde_json::from_value(params.clone()) {
                Ok(p) => Self::TurnCompleted(p),
                Err(_) => Self::Unknown {
                    method: method.to_string(),
                    params,
                },
            },
            "error" => match serde_json::from_value(params.clone()) {
                Ok(p) => Self::Error(p),
                Err(_) => Self::Unknown {
                    method: method.to_string(),
                    params,
                },
            },
            "item/agentMessage/delta" => match serde_json::from_value(params.clone()) {
                Ok(p) => Self::AgentMessageDelta(p),
                Err(_) => Self::Unknown {
                    method: method.to_string(),
                    params,
                },
            },
            "item/agentMessage" => match serde_json::from_value(params.clone()) {
                Ok(p) => Self::AgentMessage(p),
                Err(_) => Self::Unknown {
                    method: method.to_string(),
                    params,
                },
            },
            m if m.starts_with("item/toolCall") => Self::ToolCall {
                method: method.to_string(),
                params,
            },
            _ => Self::Unknown {
                method: method.to_string(),
                params,
            },
        }
    }
}

/// Params for `thread/started`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartedParams {
    /// Codex thread identifier.
    pub thread_id: String,
}

/// Params for `turn/started`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartedParams {
    /// Codex thread identifier.
    pub thread_id: String,
    /// Turn identifier.
    pub turn_id: String,
}

/// Params for `turn/completed`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnCompletedParams {
    /// Codex thread identifier.
    pub thread_id: String,
    /// Turn identifier.
    pub turn_id: String,
    /// Outcome status — typically `"succeeded"` or `"failed"`.
    pub status: String,
    /// Optional error string when `status == "failed"`.
    pub error: Option<String>,
}

/// Params for a server-emitted `error` notification.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorParams {
    /// Human-readable error message.
    pub message: String,
    /// Whether Codex intends to retry. Defaults to `false` when missing.
    #[serde(default)]
    pub will_retry: bool,
    /// Optional thread identifier scope. Some errors are global.
    pub thread_id: Option<String>,
}

/// Params for `item/agentMessage/delta`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessageDeltaParams {
    /// Codex thread identifier.
    pub thread_id: String,
    /// Turn identifier.
    pub turn_id: String,
    /// Appended text fragment.
    pub text_delta: String,
}

/// Params for a completed `item/agentMessage`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMessageParams {
    /// Codex thread identifier.
    pub thread_id: String,
    /// Turn identifier.
    pub turn_id: String,
    /// Full assistant message text.
    pub text: String,
}

// ---------------------------------------------------------------------------
// Server-initiated requests (have id, require response)
// ---------------------------------------------------------------------------

/// A server-initiated request that expects a response. Callers resolve it
/// via `JsonRpcChild::respond` with the original id and an
/// [`ApprovalResponse`] payload.
#[derive(Debug, Clone)]
pub enum ServerRequestMessage {
    /// `item/commandExecution/requestApproval`.
    CommandExecutionApproval(Value),
    /// `item/fileChange/requestApproval`.
    FileChangeApproval(Value),
    /// `item/fileRead/requestApproval`.
    FileReadApproval(Value),
    /// `item/tool/requestUserInput`.
    UserInputRequest(Value),
    /// Unknown method — treated as an approval-shaped request so the
    /// runtime can still resolve it with a `cancel` decision if needed.
    Unknown {
        /// Original method string.
        method: String,
        /// Raw payload.
        params: Value,
    },
}

impl ServerRequestMessage {
    /// Classify an incoming server request by method name.
    pub fn from_raw(method: &str, params: Value) -> Self {
        match method {
            "item/commandExecution/requestApproval" => Self::CommandExecutionApproval(params),
            "item/fileChange/requestApproval" => Self::FileChangeApproval(params),
            "item/fileRead/requestApproval" => Self::FileReadApproval(params),
            "item/tool/requestUserInput" => Self::UserInputRequest(params),
            _ => Self::Unknown {
                method: method.to_string(),
                params,
            },
        }
    }

    /// `request_kind` string fed into
    /// [`ProviderRuntimeEvent::RequestOpened`](crate::agent_provider::ProviderRuntimeEvent::RequestOpened).
    pub fn request_kind(&self) -> &str {
        match self {
            Self::CommandExecutionApproval(_) => "command",
            Self::FileChangeApproval(_) => "file-change",
            Self::FileReadApproval(_) => "file-read",
            Self::UserInputRequest(_) => "user-input",
            Self::Unknown { .. } => "unknown",
        }
    }

    /// Raw payload the adapter forwards verbatim to the UI.
    pub fn payload(&self) -> &Value {
        match self {
            Self::CommandExecutionApproval(v)
            | Self::FileChangeApproval(v)
            | Self::FileReadApproval(v)
            | Self::UserInputRequest(v) => v,
            Self::Unknown { params, .. } => params,
        }
    }
}

// ---------------------------------------------------------------------------
// Approval response (client → server, in reply to a server-initiated req)
// ---------------------------------------------------------------------------

/// Response payload we send to resolve any of the four approval-style
/// server-initiated requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalResponse {
    /// Literal decision string — `"allow"`, `"allowForSession"`,
    /// `"deny"`, or `"cancel"`.
    pub decision: String,
    /// Optional edited input the user wants applied in place of the
    /// agent's original tool input. Only meaningful when
    /// `decision == "allow"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_input: Option<Value>,
    /// Optional human-visible rejection reason. Only meaningful when
    /// `decision == "deny"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl From<ApprovalDecision> for ApprovalResponse {
    fn from(value: ApprovalDecision) -> Self {
        match value {
            ApprovalDecision::Allow { updated_input } => Self {
                decision: "allow".to_string(),
                updated_input,
                message: None,
            },
            ApprovalDecision::AllowForSession => Self {
                decision: "allowForSession".to_string(),
                updated_input: None,
                message: None,
            },
            ApprovalDecision::Deny { message } => Self {
                decision: "deny".to_string(),
                updated_input: None,
                message: Some(message),
            },
            ApprovalDecision::Cancel => Self {
                decision: "cancel".to_string(),
                updated_input: None,
                message: None,
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

    #[test]
    fn thread_start_response_parses_nested_shape() {
        let v = json!({ "thread": { "id": "t-123" } });
        let r: ThreadStartResponse = serde_json::from_value(v).unwrap();
        assert_eq!(r.thread_id(), "t-123");
    }

    #[test]
    fn thread_start_response_parses_flat_shape() {
        let v = json!({ "threadId": "t-456" });
        let r: ThreadStartResponse = serde_json::from_value(v).unwrap();
        assert_eq!(r.thread_id(), "t-456");
    }

    #[test]
    fn turn_input_text_serializes_with_type_tag() {
        let item = TurnInputItem::Text {
            text: "hello".into(),
            text_elements: vec![],
        };
        let v = serde_json::to_value(&item).unwrap();
        assert_eq!(v["type"], "text");
        assert_eq!(v["text"], "hello");
        assert!(v["text_elements"].is_array());
    }

    #[test]
    fn turn_input_image_serializes_with_type_tag() {
        let item = TurnInputItem::Image {
            url: "data:image/png;base64,xxx".into(),
        };
        let v = serde_json::to_value(&item).unwrap();
        assert_eq!(v["type"], "image");
        assert_eq!(v["url"], "data:image/png;base64,xxx");
    }

    #[test]
    fn approval_response_from_allow_omits_message() {
        let r = ApprovalResponse::from(ApprovalDecision::Allow {
            updated_input: Some(json!({"x": 1})),
        });
        assert_eq!(r.decision, "allow");
        assert_eq!(r.updated_input, Some(json!({"x": 1})));
        assert!(r.message.is_none());
    }

    #[test]
    fn approval_response_from_deny_carries_message() {
        let r = ApprovalResponse::from(ApprovalDecision::Deny {
            message: "no".into(),
        });
        assert_eq!(r.decision, "deny");
        assert_eq!(r.message.as_deref(), Some("no"));
        assert!(r.updated_input.is_none());
    }

    #[test]
    fn approval_response_serializes_skipping_none_fields() {
        let r = ApprovalResponse::from(ApprovalDecision::Cancel);
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["decision"], "cancel");
        assert!(v.get("updatedInput").is_none());
        assert!(v.get("message").is_none());
    }

    #[test]
    fn notification_from_raw_thread_started() {
        let m = NotificationMessage::from_raw(
            "thread/started",
            json!({"threadId": "thr-1"}),
        );
        match m {
            NotificationMessage::ThreadStarted(p) => assert_eq!(p.thread_id, "thr-1"),
            _ => panic!("expected ThreadStarted"),
        }
    }

    #[test]
    fn notification_from_raw_tool_call_keeps_method() {
        let m = NotificationMessage::from_raw(
            "item/toolCall/started",
            json!({"anything": true}),
        );
        match m {
            NotificationMessage::ToolCall { method, .. } => {
                assert_eq!(method, "item/toolCall/started")
            }
            _ => panic!("expected ToolCall"),
        }
    }

    #[test]
    fn notification_from_raw_unknown_method() {
        let m = NotificationMessage::from_raw("foo/bar", json!({"k": "v"}));
        match m {
            NotificationMessage::Unknown { method, params } => {
                assert_eq!(method, "foo/bar");
                assert_eq!(params, json!({"k": "v"}));
            }
            _ => panic!("expected Unknown"),
        }
    }

    #[test]
    fn notification_from_raw_malformed_params_falls_through_to_unknown() {
        // Missing `threadId` field on thread/started should land us in Unknown
        // rather than panic.
        let m = NotificationMessage::from_raw("thread/started", json!({"wrong": 1}));
        match m {
            NotificationMessage::Unknown { method, .. } => {
                assert_eq!(method, "thread/started")
            }
            _ => panic!("expected Unknown"),
        }
    }

    #[test]
    fn server_request_classification() {
        let r = ServerRequestMessage::from_raw(
            "item/commandExecution/requestApproval",
            json!({"cmd": "ls"}),
        );
        assert_eq!(r.request_kind(), "command");

        let r = ServerRequestMessage::from_raw(
            "item/fileChange/requestApproval",
            json!({}),
        );
        assert_eq!(r.request_kind(), "file-change");

        let r = ServerRequestMessage::from_raw(
            "item/fileRead/requestApproval",
            json!({}),
        );
        assert_eq!(r.request_kind(), "file-read");

        let r = ServerRequestMessage::from_raw("item/tool/requestUserInput", json!({}));
        assert_eq!(r.request_kind(), "user-input");

        let r = ServerRequestMessage::from_raw("foo/baz", json!({}));
        assert_eq!(r.request_kind(), "unknown");
    }
}
