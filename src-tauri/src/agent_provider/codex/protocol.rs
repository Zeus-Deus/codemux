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
/// Mirrors the canonical SDK shape (`V2GetAccountResponse`). `account` is
/// optional, while `requires_openai_auth` describes the active model
/// provider rather than the current login state. A normal ChatGPT login, for
/// example, returns both `account: Some(...)` and
/// `requires_openai_auth: true`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountReadResponse {
    /// Account info — `None` when no user is logged in.
    #[serde(default)]
    pub account: Option<AccountInfo>,
    /// Whether the active model provider needs OpenAI credentials. This is
    /// not itself an authentication-status bit: it remains `true` after a
    /// successful ChatGPT or API-key login.
    #[serde(default)]
    pub requires_openai_auth: bool,
}

impl AccountReadResponse {
    /// Whether Codex cannot serve the active provider until the user logs in.
    ///
    /// The app-server contract deliberately represents three useful states:
    ///
    /// * account present + OpenAI auth required: logged in to ChatGPT/API key;
    /// * no account + OpenAI auth required: login is missing;
    /// * no account + OpenAI auth not required: local/custom provider usable.
    pub fn needs_login(&self) -> bool {
        self.requires_openai_auth && self.account.is_none()
    }
}

/// Subset of the account info we care about. The SDK exposes much more
/// (rate limits, plan types, etc.); we only need the auth-mode tag.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInfo {
    /// Tag string — typically `"chatgpt"`, `"apiKey"`, or
    /// `"chatgptDeviceCode"`. Drives picker hints later.
    #[serde(default, rename = "type")]
    pub account_type: Option<String>,
    /// Subscription plan label when known (e.g. `"plus"`, `"pro"`).
    #[serde(default)]
    pub plan_type: Option<String>,
}

// ---------------------------------------------------------------------------
// model/list
// ---------------------------------------------------------------------------

/// Params for `model/list`. All fields optional — calling with `{}` is the
/// canonical "give me everything" form.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelListParams {
    /// Pagination cursor returned by a previous call. `None` for the
    /// first page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Whether to include hidden / preview models.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_hidden: Option<bool>,
    /// Page size cap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Response to `model/list`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelListResponse {
    /// One entry per model the account has access to.
    pub data: Vec<ModelEntry>,
    /// Opaque continuation token. `None` when the response is complete.
    #[serde(default)]
    pub next_cursor: Option<String>,
}

/// One model the SDK reports.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelEntry {
    /// Stable identifier — e.g. `"gpt-5.4"`, `"gpt-5.4-mini"`. This is
    /// what gets passed back as the `model` field on `thread/start` /
    /// `turn/start`.
    pub id: String,
    /// Underlying model string (often duplicates `id` but kept distinct
    /// in the SDK so we mirror the field).
    #[serde(default)]
    pub model: String,
    /// Human-readable label rendered by the picker.
    #[serde(default)]
    pub display_name: String,
    /// One-line description — surfaced as the picker subtitle.
    #[serde(default)]
    pub description: String,
    /// True when the model is hidden from default UIs (still selectable
    /// when explicitly requested, but the picker leaves it out).
    #[serde(default)]
    pub hidden: bool,
    /// True for the SDK's recommended default model. Used to seed the
    /// picker selection on first open.
    #[serde(default)]
    pub is_default: bool,
    /// Default reasoning effort for this model — `"none"`, `"minimal"`,
    /// `"low"`, `"medium"`, `"high"`, `"xhigh"`.
    #[serde(default)]
    pub default_reasoning_effort: String,
    /// Reasoning effort options the model honours, with the per-effort
    /// description the SDK ships.
    #[serde(default)]
    pub supported_reasoning_efforts: Vec<ReasoningEffortOption>,
    /// Input modalities — `["text"]`, `["text", "image"]`, etc.
    /// Determines the picker's image-attachment chip.
    #[serde(default)]
    pub input_modalities: Vec<String>,
    /// Extra speed tiers (`"fast"`, `"flex"`, ...) the model supports.
    /// Maps to the picker's fast-mode toggle when `"fast"` is present.
    #[serde(default)]
    pub additional_speed_tiers: Vec<String>,
}

/// One element of [`ModelEntry::supported_reasoning_efforts`].
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningEffortOption {
    /// Effort level identifier — one of `"none" | "minimal" | "low" |
    /// "medium" | "high" | "xhigh"`.
    pub reasoning_effort: String,
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
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
    AgentMessageDelta(DeltaParams),
    /// `item/reasoning/textDelta` — streaming reasoning text.
    ReasoningTextDelta(ReasoningTextDeltaParams),
    /// `item/reasoning/summaryTextDelta` — streaming reasoning summary.
    ReasoningSummaryTextDelta(ReasoningSummaryTextDeltaParams),
    /// `item/reasoning/summaryPartAdded` — new summary segment marker.
    ReasoningSummaryPartAdded(ReasoningSummaryPartAddedParams),
    /// `item/commandExecution/outputDelta` — bash stdout/stderr chunk.
    CommandExecutionOutputDelta(DeltaParams),
    /// `item/commandExecution/terminalInteraction` — terminal IO event
    /// (typically stdin echo for an interactive command). Surfaced as a
    /// runtime warning since the chat UI doesn't render an interactive
    /// terminal yet.
    TerminalInteraction(TerminalInteractionParams),
    /// `item/fileChange/outputDelta` — file edit text chunk.
    FileChangeOutputDelta(DeltaParams),
    /// `item/fileChange/patchUpdated` — full updated patch payload.
    FileChangePatchUpdated(FileChangePatchUpdatedParams),
    /// `item/mcpToolCall/progress` — MCP tool call status text.
    McpToolCallProgress(McpToolCallProgressParams),
    /// `item/plan/delta` — plan content streaming chunk.
    PlanDelta(DeltaParams),
    /// `item/started` — an item has just been created. The `item.type`
    /// tag indicates whether it's a tool call, reasoning block, etc.
    ItemStarted(ItemEnvelope),
    /// `item/completed` — an item has finalised; payload carries the
    /// full final-form data (text, tool result, file change, etc.).
    ItemCompleted(ItemEnvelope),
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
    /// Build a typed notification from the raw method + params pair.
    /// Never returns an error — unrecognised methods OR malformed payloads
    /// fall through to [`NotificationMessage::Unknown`].
    pub fn from_raw(method: &str, params: Value) -> Self {
        // Helper to keep the match arms tight: try to decode `params` into
        // the variant's params struct; if the decode fails, fall through
        // to `Unknown` so a payload-shape drift never panics the adapter.
        macro_rules! decode {
            ($variant:ident) => {{
                match serde_json::from_value(params.clone()) {
                    Ok(p) => Self::$variant(p),
                    Err(_) => Self::Unknown {
                        method: method.to_string(),
                        params,
                    },
                }
            }};
        }

        match method {
            "thread/started" => decode!(ThreadStarted),
            "turn/started" => decode!(TurnStarted),
            "turn/completed" => decode!(TurnCompleted),
            "error" => decode!(Error),
            "item/agentMessage/delta" => decode!(AgentMessageDelta),
            "item/reasoning/textDelta" => decode!(ReasoningTextDelta),
            "item/reasoning/summaryTextDelta" => decode!(ReasoningSummaryTextDelta),
            "item/reasoning/summaryPartAdded" => decode!(ReasoningSummaryPartAdded),
            "item/commandExecution/outputDelta" => decode!(CommandExecutionOutputDelta),
            "item/commandExecution/terminalInteraction" => decode!(TerminalInteraction),
            "item/fileChange/outputDelta" => decode!(FileChangeOutputDelta),
            "item/fileChange/patchUpdated" => decode!(FileChangePatchUpdated),
            "item/mcpToolCall/progress" => decode!(McpToolCallProgress),
            "item/plan/delta" => decode!(PlanDelta),
            "item/started" => decode!(ItemStarted),
            "item/completed" => decode!(ItemCompleted),
            _ => Self::Unknown {
                method: method.to_string(),
                params,
            },
        }
    }
}

/// Params for `thread/started`.
///
/// # Wire-drift tolerance
///
/// Two shapes are observed in the wild and both must decode:
///
/// * **Legacy flat** — `{ "threadId": "..." }`.
/// * **v2 nested** — `{ "thread": { "id": "...", "parentThreadId": "...",
///   "agentNickname": "...", "agentRole": "..." } }`.
///
/// The nested shape additionally carries subagent identity: when Codex
/// spawns a sub-agent thread, `parentThreadId` points at the spawning
/// thread and `agentNickname` / `agentRole` name the sub-agent. Those
/// fields let the adapter demux child-thread notifications into the
/// canonical subagent view. Both shapes are decoded via an untagged
/// helper enum; a malformed payload (neither `threadId` nor `thread.id`)
/// fails to deserialize and falls through to
/// [`NotificationMessage::Unknown`].
#[derive(Debug, Clone)]
pub struct ThreadStartedParams {
    /// Codex thread identifier of the thread that started.
    pub thread_id: String,
    /// Parent thread id — set only when this thread is a spawned
    /// sub-agent (v2 nested shape).
    pub parent_thread_id: Option<String>,
    /// Random unique nickname assigned to an AgentControl-spawned
    /// sub-agent (v2 nested shape).
    pub agent_nickname: Option<String>,
    /// Role (`agent_role`) assigned to an AgentControl-spawned sub-agent
    /// (v2 nested shape).
    pub agent_role: Option<String>,
}

/// Untagged decode helper for [`ThreadStartedParams`]. Tries the v2
/// nested shape first, then the legacy flat shape.
#[derive(Deserialize)]
#[serde(untagged)]
enum ThreadStartedWire {
    Nested { thread: ThreadStartedThread },
    Flat {
        #[serde(rename = "threadId")]
        thread_id: String,
    },
}

/// Subset of the v2 `Thread` object we care about for demux/identity.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadStartedThread {
    id: String,
    #[serde(default)]
    parent_thread_id: Option<String>,
    #[serde(default)]
    agent_nickname: Option<String>,
    #[serde(default)]
    agent_role: Option<String>,
}

impl<'de> Deserialize<'de> for ThreadStartedParams {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(match ThreadStartedWire::deserialize(deserializer)? {
            ThreadStartedWire::Nested { thread } => Self {
                thread_id: thread.id,
                parent_thread_id: thread.parent_thread_id,
                agent_nickname: thread.agent_nickname,
                agent_role: thread.agent_role,
            },
            ThreadStartedWire::Flat { thread_id } => Self {
                thread_id,
                parent_thread_id: None,
                agent_nickname: None,
                agent_role: None,
            },
        })
    }
}

/// Shared subset of the v2 `Turn` object used by both turn notifications.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TurnObj {
    id: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    error: Option<TurnErrorObj>,
    #[serde(default)]
    duration_ms: Option<u64>,
}

/// The v2 `TurnError` object; we only surface its `message`.
#[derive(Deserialize)]
struct TurnErrorObj {
    message: String,
}

/// Params for `turn/started`.
///
/// Decodes BOTH the legacy flat `{ threadId, turnId }` shape AND the v2
/// nested `{ threadId, turn: { id, status, ... } }` shape.
#[derive(Debug, Clone)]
pub struct TurnStartedParams {
    /// Codex thread identifier (may be the parent thread or a sub-agent
    /// child thread — the adapter demuxes on it).
    pub thread_id: String,
    /// Turn identifier.
    pub turn_id: String,
}

/// Untagged decode helper for [`TurnStartedParams`].
#[derive(Deserialize)]
#[serde(untagged)]
enum TurnStartedWire {
    Nested {
        #[serde(rename = "threadId")]
        thread_id: String,
        turn: TurnObj,
    },
    Flat {
        #[serde(rename = "threadId")]
        thread_id: String,
        #[serde(rename = "turnId")]
        turn_id: String,
    },
}

impl<'de> Deserialize<'de> for TurnStartedParams {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(match TurnStartedWire::deserialize(deserializer)? {
            TurnStartedWire::Nested { thread_id, turn } => Self {
                thread_id,
                turn_id: turn.id,
            },
            TurnStartedWire::Flat {
                thread_id,
                turn_id,
            } => Self { thread_id, turn_id },
        })
    }
}

/// Params for `turn/completed`.
///
/// Decodes BOTH the legacy flat `{ threadId, turnId, status, error }`
/// shape AND the v2 nested `{ threadId, turn: { id, status, error,
/// durationMs } }` shape. `status` is preserved verbatim; the v2 nested
/// shape reports `completed` / `interrupted` / `failed` / `inProgress`
/// where the legacy shape reports `succeeded` / `failed`, and the
/// translator accepts both vocabularies.
#[derive(Debug, Clone)]
pub struct TurnCompletedParams {
    /// Codex thread identifier.
    pub thread_id: String,
    /// Turn identifier.
    pub turn_id: String,
    /// Outcome status. Legacy: `"succeeded"` / `"failed"`. v2:
    /// `"completed"` / `"interrupted"` / `"failed"` / `"inProgress"`.
    pub status: String,
    /// Error message when the turn failed (flattened from the v2
    /// `TurnError` object or taken from the legacy flat string).
    pub error: Option<String>,
    /// Elapsed wall-clock time in milliseconds, when the v2 shape
    /// reports it. Feeds a sub-agent's `duration_ms` for child turns.
    pub duration_ms: Option<u64>,
}

/// Untagged decode helper for [`TurnCompletedParams`].
#[derive(Deserialize)]
#[serde(untagged)]
enum TurnCompletedWire {
    Nested {
        #[serde(rename = "threadId")]
        thread_id: String,
        turn: TurnObj,
    },
    Flat {
        #[serde(rename = "threadId")]
        thread_id: String,
        #[serde(rename = "turnId")]
        turn_id: String,
        status: String,
        #[serde(default)]
        error: Option<String>,
    },
}

impl<'de> Deserialize<'de> for TurnCompletedParams {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(match TurnCompletedWire::deserialize(deserializer)? {
            TurnCompletedWire::Nested { thread_id, turn } => Self {
                thread_id,
                turn_id: turn.id,
                status: turn.status.unwrap_or_default(),
                error: turn.error.map(|e| e.message),
                duration_ms: turn.duration_ms,
            },
            TurnCompletedWire::Flat {
                thread_id,
                turn_id,
                status,
                error,
            } => Self {
                thread_id,
                turn_id,
                status,
                error,
                duration_ms: None,
            },
        })
    }
}

/// Decoded `collabAgentToolCall` thread item (the multi-agent launch /
/// lifecycle tool call). Emitted on the parent thread when it spawns,
/// waits on, or closes a sub-agent. The adapter maps it to
/// [`SubagentUpdated`](crate::agent_provider::ProviderRuntimeEvent::SubagentUpdated)
/// and suppresses its generic tool rendering.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollabAgentToolCallItem {
    /// Unique identifier for this collab tool call (the spawning
    /// `parent_item_id`).
    pub id: String,
    /// Which collab tool ran: `spawnAgent` | `sendInput` | `resumeAgent`
    /// | `wait` | `closeAgent`.
    pub tool: String,
    /// Current status of the collab tool call: `inProgress` |
    /// `completed` | `failed`.
    pub status: String,
    /// Thread id of the agent issuing the collab request.
    #[serde(default)]
    pub sender_thread_id: Option<String>,
    /// Thread ids of the receiving agents. For `spawnAgent` this is the
    /// newly spawned child thread id(s).
    #[serde(default)]
    pub receiver_thread_ids: Vec<String>,
    /// Last known status of the target agents, keyed by thread id.
    /// `BTreeMap` for deterministic iteration in tests.
    #[serde(default)]
    pub agents_states: std::collections::BTreeMap<String, CollabAgentStateObj>,
    /// Model requested for the spawned agent, when applicable.
    #[serde(default)]
    pub model: Option<String>,
    /// Prompt text sent as part of the collab tool call, when available.
    #[serde(default)]
    pub prompt: Option<String>,
}

/// Per-agent state block inside [`CollabAgentToolCallItem::agents_states`].
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollabAgentStateObj {
    /// One of `pendingInit` | `running` | `interrupted` | `completed` |
    /// `errored` | `shutdown` | `notFound`.
    pub status: String,
    /// Optional live status message — feeds the subagent activity line.
    #[serde(default)]
    pub message: Option<String>,
}

/// Decoded `subAgentActivity` thread item — a cheap status tick emitted
/// when a sub-agent starts, is interacted with, or is interrupted.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubAgentActivityItem {
    /// Unique identifier for this activity item.
    pub id: String,
    /// Thread id of the sub-agent this activity refers to.
    pub agent_thread_id: String,
    /// Path to the sub-agent, when reported.
    #[serde(default)]
    pub agent_path: Option<String>,
    /// One of `started` | `interacted` | `interrupted`.
    pub kind: String,
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

/// Shared shape for every "delta" notification in the SDK
/// (`agentMessage`, `commandExecution`, `fileChange`, `plan`). All four
/// carry exactly `{ delta, itemId, threadId, turnId }`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeltaParams {
    /// Text fragment that was just emitted.
    pub delta: String,
    /// Identifier of the item the delta belongs to (a single message,
    /// command execution, file change, or plan item). Used by the UI to
    /// thread deltas onto the right card.
    pub item_id: String,
    /// Codex thread identifier.
    pub thread_id: String,
    /// Turn identifier.
    pub turn_id: String,
}

/// Params for `item/reasoning/textDelta`. Same as [`DeltaParams`] but
/// with an extra `content_index` for ordering inside multi-segment
/// reasoning streams.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningTextDeltaParams {
    /// Text fragment.
    pub delta: String,
    /// Stable index of this content segment within the reasoning item.
    pub content_index: i64,
    /// Identifier of the reasoning item.
    pub item_id: String,
    /// Codex thread identifier.
    pub thread_id: String,
    /// Turn identifier.
    pub turn_id: String,
}

/// Params for `item/reasoning/summaryTextDelta` — like
/// [`ReasoningTextDeltaParams`] but indexes summary parts instead of
/// content parts.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningSummaryTextDeltaParams {
    /// Text fragment.
    pub delta: String,
    /// Stable index of this summary segment.
    pub summary_index: i64,
    /// Identifier of the reasoning item.
    pub item_id: String,
    /// Codex thread identifier.
    pub thread_id: String,
    /// Turn identifier.
    pub turn_id: String,
}

/// Params for `item/reasoning/summaryPartAdded` — emitted when the model
/// starts a new reasoning-summary segment. No text payload; the
/// segment's content streams in via subsequent
/// `summaryTextDelta` events.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningSummaryPartAddedParams {
    /// Identifier of the reasoning item.
    pub item_id: String,
    /// Stable index of the new summary segment.
    pub summary_index: i64,
    /// Codex thread identifier.
    pub thread_id: String,
    /// Turn identifier.
    pub turn_id: String,
}

/// Params for `item/commandExecution/terminalInteraction`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInteractionParams {
    /// Item identifier.
    pub item_id: String,
    /// PTY process identifier.
    pub process_id: String,
    /// Stdin chunk that was just delivered to the process.
    pub stdin: String,
    /// Codex thread identifier.
    pub thread_id: String,
    /// Turn identifier.
    pub turn_id: String,
}

/// Params for `item/fileChange/patchUpdated`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChangePatchUpdatedParams {
    /// File-change list. Surfaced verbatim into the runtime event so the
    /// UI can render the patch preview without re-parsing.
    pub changes: Value,
    /// Item identifier.
    pub item_id: String,
    /// Codex thread identifier.
    pub thread_id: String,
    /// Turn identifier.
    pub turn_id: String,
}

/// Params for `item/mcpToolCall/progress`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolCallProgressParams {
    /// Item identifier.
    pub item_id: String,
    /// Status text (typically a short human-readable progress note like
    /// `"connecting"` / `"running"`).
    pub message: String,
    /// Codex thread identifier.
    pub thread_id: String,
    /// Turn identifier.
    pub turn_id: String,
}

/// Shared envelope for `item/started` and `item/completed`. The `item`
/// payload is left as raw JSON — the union has 14 variants and the chat
/// UI only needs the type tag plus a few common fields, so we let the
/// translate layer cherry-pick what it cares about per-type.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemEnvelope {
    /// Full item payload — `{ type, id, ...type-specific fields }`.
    pub item: Value,
    /// Codex thread identifier.
    pub thread_id: String,
    /// Turn identifier.
    pub turn_id: String,
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
    /// `item/permissions/requestApproval` — newer SDK permission flow
    /// that carries a structured permission profile (file system, network,
    /// etc.) instead of a single command. Forwarded verbatim to the UI.
    PermissionsApproval(Value),
    /// `item/tool/call` — server-initiated dynamic tool invocation. The
    /// caller is expected to execute the tool and reply with the result.
    /// We don't synthesize tool execution today; surface as a request and
    /// let the user/UI decide.
    ToolCall(Value),
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
            "item/permissions/requestApproval" => Self::PermissionsApproval(params),
            "item/tool/call" => Self::ToolCall(params),
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
            Self::PermissionsApproval(_) => "permissions",
            Self::ToolCall(_) => "tool-call",
            Self::Unknown { .. } => "unknown",
        }
    }

    /// Raw payload the adapter forwards verbatim to the UI.
    pub fn payload(&self) -> &Value {
        match self {
            Self::CommandExecutionApproval(v)
            | Self::FileChangeApproval(v)
            | Self::FileReadApproval(v)
            | Self::UserInputRequest(v)
            | Self::PermissionsApproval(v)
            | Self::ToolCall(v) => v,
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
            ApprovalDecision::Allow {
                updated_input,
                // Codex has no analogue to Claude's updatedPermissions —
                // the "always allow" path will surface through Codex's
                // sandbox policy instead. Drop the rules on the floor.
                updated_permissions: _,
            } => Self {
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
    fn account_with_openai_auth_required_is_logged_in() {
        let response: AccountReadResponse = serde_json::from_value(json!({
            "account": {"type": "chatgpt", "planType": "pro"},
            "requiresOpenaiAuth": true
        }))
        .unwrap();
        assert!(!response.needs_login());
    }

    #[test]
    fn missing_account_with_openai_auth_required_needs_login() {
        let response: AccountReadResponse = serde_json::from_value(json!({
            "account": null,
            "requiresOpenaiAuth": true
        }))
        .unwrap();
        assert!(response.needs_login());
    }

    #[test]
    fn provider_without_openai_auth_can_run_without_account() {
        let response: AccountReadResponse = serde_json::from_value(json!({
            "account": null,
            "requiresOpenaiAuth": false
        }))
        .unwrap();
        assert!(!response.needs_login());
    }

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

    // Stage 6 — full TurnStartParams round-trip with images. Lives
    // alongside the per-item tests above so we lock in the exact
    // outer wire shape (image-before-text, data: URI form) the
    // Codex app-server expects in production.
    #[test]
    fn turn_start_input_array_places_images_before_text() {
        let params = TurnStartParams {
            thread_id: "t-codex".into(),
            input: vec![
                TurnInputItem::Image {
                    url: "data:image/png;base64,AAA".into(),
                },
                TurnInputItem::Image {
                    url: "data:image/jpeg;base64,BBB".into(),
                },
                TurnInputItem::Text {
                    text: "what's in these?".into(),
                    text_elements: vec![],
                },
            ],
            model: Some("gpt-5.4".into()),
            service_tier: None,
            effort: None,
            collaboration_mode: None,
        };
        let v = serde_json::to_value(&params).unwrap();
        let arr = v["input"].as_array().expect("input must be array");
        assert_eq!(arr.len(), 3);
        // Strict order: image, image, then text.
        assert_eq!(arr[0]["type"], "image");
        assert_eq!(arr[1]["type"], "image");
        assert_eq!(arr[2]["type"], "text");
        assert_eq!(arr[0]["url"], "data:image/png;base64,AAA");
        assert_eq!(arr[2]["text"], "what's in these?");
    }

    #[test]
    fn turn_start_input_text_only_path_keeps_legacy_shape() {
        // Text-only regression check: when no images are passed the
        // input array carries a single Text item — same shape as the
        // pre-Stage-6 contract so we don't accidentally regress
        // Codex chats that never touch images.
        let params = TurnStartParams {
            thread_id: "t-codex".into(),
            input: vec![TurnInputItem::Text {
                text: "hi".into(),
                text_elements: vec![],
            }],
            model: None,
            service_tier: None,
            effort: None,
            collaboration_mode: None,
        };
        let v = serde_json::to_value(&params).unwrap();
        let arr = v["input"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["type"], "text");
    }

    #[test]
    fn approval_response_from_allow_omits_message() {
        let r = ApprovalResponse::from(ApprovalDecision::Allow {
            updated_input: Some(json!({"x": 1})),
            updated_permissions: None,
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
    fn notification_from_raw_legacy_tool_call_falls_through_to_unknown() {
        // The pre-Stage-9 adapter pattern-matched on `item/toolCall/*`,
        // a method namespace that does not exist in the SDK manifest.
        // Real Codex emits `item/started` + `item/completed` plus the
        // typed delta methods. Anything still using the legacy name is
        // surfaced as Unknown so it stays diagnosable.
        let m = NotificationMessage::from_raw(
            "item/toolCall/started",
            json!({"anything": true}),
        );
        match m {
            NotificationMessage::Unknown { method, .. } => {
                assert_eq!(method, "item/toolCall/started")
            }
            _ => panic!("expected Unknown"),
        }
    }

    #[test]
    fn notification_from_raw_agent_message_delta_decodes_real_sdk_fields() {
        let m = NotificationMessage::from_raw(
            "item/agentMessage/delta",
            json!({
                "delta": "hi",
                "itemId": "i-1",
                "threadId": "c1",
                "turnId": "t1"
            }),
        );
        match m {
            NotificationMessage::AgentMessageDelta(p) => {
                assert_eq!(p.delta, "hi");
                assert_eq!(p.item_id, "i-1");
            }
            _ => panic!("expected AgentMessageDelta"),
        }
    }

    #[test]
    fn notification_from_raw_item_completed_carries_item_payload() {
        let m = NotificationMessage::from_raw(
            "item/completed",
            json!({
                "threadId": "c1",
                "turnId": "t1",
                "item": { "type": "agentMessage", "id": "am-1", "text": "done" }
            }),
        );
        match m {
            NotificationMessage::ItemCompleted(env) => {
                assert_eq!(env.thread_id, "c1");
                assert_eq!(env.item.get("type").and_then(|v| v.as_str()), Some("agentMessage"));
            }
            _ => panic!("expected ItemCompleted"),
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

    // ── Wire-drift tolerance: thread/started ──

    #[test]
    fn thread_started_decodes_legacy_flat_shape() {
        let m = NotificationMessage::from_raw("thread/started", json!({"threadId": "flat-1"}));
        match m {
            NotificationMessage::ThreadStarted(p) => {
                assert_eq!(p.thread_id, "flat-1");
                assert!(p.parent_thread_id.is_none());
                assert!(p.agent_nickname.is_none());
                assert!(p.agent_role.is_none());
            }
            _ => panic!("expected ThreadStarted"),
        }
    }

    #[test]
    fn thread_started_decodes_v2_nested_shape_with_subagent_identity() {
        let m = NotificationMessage::from_raw(
            "thread/started",
            json!({
                "thread": {
                    "id": "child-1",
                    "parentThreadId": "parent-1",
                    "agentNickname": "Explore",
                    "agentRole": "explore",
                    // extra fields the real server sends are ignored:
                    "cwd": "/tmp",
                    "status": "running",
                    "turns": []
                }
            }),
        );
        match m {
            NotificationMessage::ThreadStarted(p) => {
                assert_eq!(p.thread_id, "child-1");
                assert_eq!(p.parent_thread_id.as_deref(), Some("parent-1"));
                assert_eq!(p.agent_nickname.as_deref(), Some("Explore"));
                assert_eq!(p.agent_role.as_deref(), Some("explore"));
            }
            _ => panic!("expected ThreadStarted"),
        }
    }

    // ── Wire-drift tolerance: turn/started ──

    #[test]
    fn turn_started_decodes_legacy_flat_shape() {
        let m = NotificationMessage::from_raw(
            "turn/started",
            json!({"threadId": "c1", "turnId": "t1"}),
        );
        match m {
            NotificationMessage::TurnStarted(p) => {
                assert_eq!(p.thread_id, "c1");
                assert_eq!(p.turn_id, "t1");
            }
            _ => panic!("expected TurnStarted"),
        }
    }

    #[test]
    fn turn_started_decodes_v2_nested_shape() {
        let m = NotificationMessage::from_raw(
            "turn/started",
            json!({"threadId": "c1", "turn": {"id": "t1", "status": "inProgress", "items": []}}),
        );
        match m {
            NotificationMessage::TurnStarted(p) => {
                assert_eq!(p.thread_id, "c1");
                assert_eq!(p.turn_id, "t1");
            }
            _ => panic!("expected TurnStarted"),
        }
    }

    // ── Wire-drift tolerance: turn/completed ──

    #[test]
    fn turn_completed_decodes_legacy_flat_shape() {
        let m = NotificationMessage::from_raw(
            "turn/completed",
            json!({"threadId": "c1", "turnId": "t1", "status": "succeeded"}),
        );
        match m {
            NotificationMessage::TurnCompleted(p) => {
                assert_eq!(p.thread_id, "c1");
                assert_eq!(p.turn_id, "t1");
                assert_eq!(p.status, "succeeded");
                assert!(p.error.is_none());
                assert!(p.duration_ms.is_none());
            }
            _ => panic!("expected TurnCompleted"),
        }
    }

    #[test]
    fn turn_completed_decodes_legacy_flat_error_string() {
        let m = NotificationMessage::from_raw(
            "turn/completed",
            json!({"threadId": "c1", "turnId": "t1", "status": "failed", "error": "boom"}),
        );
        match m {
            NotificationMessage::TurnCompleted(p) => {
                assert_eq!(p.status, "failed");
                assert_eq!(p.error.as_deref(), Some("boom"));
            }
            _ => panic!("expected TurnCompleted"),
        }
    }

    #[test]
    fn turn_completed_decodes_v2_nested_shape_with_duration_and_error() {
        let m = NotificationMessage::from_raw(
            "turn/completed",
            json!({
                "threadId": "c1",
                "turn": {
                    "id": "t1",
                    "status": "failed",
                    "durationMs": 4321,
                    "error": {"message": "kaboom", "additionalDetails": null},
                    "items": []
                }
            }),
        );
        match m {
            NotificationMessage::TurnCompleted(p) => {
                assert_eq!(p.thread_id, "c1");
                assert_eq!(p.turn_id, "t1");
                assert_eq!(p.status, "failed");
                assert_eq!(p.error.as_deref(), Some("kaboom"));
                assert_eq!(p.duration_ms, Some(4321));
            }
            _ => panic!("expected TurnCompleted"),
        }
    }

    #[test]
    fn turn_completed_v2_nested_completed_status_preserved() {
        // v2 vocabulary reports "completed" (not "succeeded"); the raw
        // string is preserved for the translator to normalize.
        let m = NotificationMessage::from_raw(
            "turn/completed",
            json!({"threadId": "c1", "turn": {"id": "t1", "status": "completed", "items": []}}),
        );
        match m {
            NotificationMessage::TurnCompleted(p) => assert_eq!(p.status, "completed"),
            _ => panic!("expected TurnCompleted"),
        }
    }

    // ── collabAgentToolCall / subAgentActivity item decode ──

    #[test]
    fn collab_agent_tool_call_item_decodes_spawn_fields() {
        let call: CollabAgentToolCallItem = serde_json::from_value(json!({
            "type": "collabAgentToolCall",
            "id": "call-1",
            "tool": "spawnAgent",
            "status": "completed",
            "senderThreadId": "parent-1",
            "receiverThreadIds": ["child-1"],
            "model": "gpt-5.4",
            "prompt": "explore the repo",
            "agentsStates": {
                "child-1": {"status": "running", "message": "reading files"}
            }
        }))
        .unwrap();
        assert_eq!(call.id, "call-1");
        assert_eq!(call.tool, "spawnAgent");
        assert_eq!(call.status, "completed");
        assert_eq!(call.sender_thread_id.as_deref(), Some("parent-1"));
        assert_eq!(call.receiver_thread_ids, vec!["child-1".to_string()]);
        assert_eq!(call.model.as_deref(), Some("gpt-5.4"));
        let st = call.agents_states.get("child-1").unwrap();
        assert_eq!(st.status, "running");
        assert_eq!(st.message.as_deref(), Some("reading files"));
    }

    #[test]
    fn sub_agent_activity_item_decodes_fields() {
        let act: SubAgentActivityItem = serde_json::from_value(json!({
            "type": "subAgentActivity",
            "id": "act-1",
            "agentThreadId": "child-1",
            "agentPath": "root/child",
            "kind": "interacted"
        }))
        .unwrap();
        assert_eq!(act.id, "act-1");
        assert_eq!(act.agent_thread_id, "child-1");
        assert_eq!(act.agent_path.as_deref(), Some("root/child"));
        assert_eq!(act.kind, "interacted");
    }
}
