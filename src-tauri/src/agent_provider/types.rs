//! Shared types used by the [`AgentProvider`](crate::agent_provider::AgentProvider)
//! trait.
//!
//! These structs and newtypes are the lingua franca between the chat runtime
//! and any concrete provider implementation. The orchestration layer treats
//! them as opaque identifiers and values so new providers can slot in without
//! touching the engine.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The set of CLI-backed coding agents the chat runtime can drive.
///
/// Serialized as lowercase strings (`"claude"`, `"codex"`, `"opencode"`) so
/// values round-trip cleanly through JSON settings and IPC surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    /// Claude Code via the `claude` CLI / Claude Agent SDK.
    Claude,
    /// Codex via the `codex app-server` JSON-RPC binary.
    Codex,
    /// OpenCode via the `opencode` HTTP server. Step 12 Stage 1 scaffold —
    /// the runtime adapter is not implemented yet and command dispatch
    /// returns a placeholder error.
    OpenCode,
}

/// Capabilities a provider declares statically so the UI can enable or hide
/// controls without probing.
///
/// Defaults to all `false`; implementations flip the fields they actually
/// support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProviderCapabilities {
    /// Model can be swapped mid-session without reconnecting.
    pub supports_mid_session_model_change: bool,
    /// Permission mode (accept-edits, bypass, plan, ...) can be changed
    /// mid-session.
    pub supports_mid_session_permission_change: bool,
    /// Tool approvals can be granted or denied synchronously while a turn is
    /// in flight (vs. fire-and-forget allow-lists).
    pub supports_synchronous_tool_approval: bool,
    /// A running turn can be interrupted without tearing the session down.
    pub supports_interrupt: bool,
    /// A previously-closed session can be resumed from an opaque cursor.
    pub supports_session_resume: bool,
}

/// Opaque identifier a provider hands back for its own internal session.
///
/// Contents depend on the provider — a Claude SDK session UUID, a Codex
/// app-server `thread_id`, etc. The runtime stores this on the thread's
/// resume state but never interprets it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderSessionId(pub String);

/// Stable identifier for a chat thread, owned by the runtime.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ThreadId(pub String);

/// Identifier for a single assistant turn within a thread.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TurnId(pub String);

/// Identifier for a provider-side request (tool approval, file change, etc.)
/// that needs an out-of-band response from the user.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(pub String);

/// A user-supplied image attachment passed alongside a turn's text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageInput {
    /// Raw bytes of the image. The runtime is responsible for persisting or
    /// re-encoding these as the provider requires.
    pub data: Vec<u8>,
    /// Media type (e.g. `"image/png"`). Provider-specific validation happens
    /// inside the adapter.
    pub media_type: String,
}

/// Parameters for starting a brand-new or resumed provider session.
///
/// Providers that do not recognise a field ignore it; the runtime always
/// passes the full structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartSessionInput {
    /// Runtime-owned thread identifier.
    pub thread_id: ThreadId,
    /// Working directory to hand the agent.
    pub cwd: PathBuf,
    /// Optional initial model identifier. Providers fall back to their
    /// default when absent.
    pub model: Option<String>,
    /// Opaque resume state previously returned by the provider. When
    /// present the provider should attempt to resume instead of starting a
    /// fresh session.
    pub resume_cursor: Option<serde_json::Value>,
    /// Initial permission mode name. String to avoid baking each provider's
    /// enum into the trait.
    pub permission_mode: Option<String>,
    /// Optional reasoning / effort level. For Claude this is session-level
    /// (restart on change). Codex uses the per-turn field on
    /// [`SendTurnInput`] instead.
    #[serde(default)]
    pub effort: Option<String>,
    /// Optional context-window selection (Claude-only today). When set to
    /// `"1m"` the Claude adapter appends the `[1m]` bracket to the model
    /// id before the SDK call.
    #[serde(default)]
    pub context_window: Option<String>,
    /// Whether to request the provider's premium fast inference tier.
    /// Capability gating happens in the UI; providers that do not expose
    /// fast mode ignore this field.
    #[serde(default)]
    pub fast_mode: bool,
    /// Optional list of extra directories the agent should be allowed to
    /// access beyond `cwd`.
    pub additional_directories: Vec<PathBuf>,
    /// Environment variables to overlay onto the spawned child, or inherit
    /// the runtime's env when `None`.
    pub env: Option<std::collections::HashMap<String, String>>,
    /// Free-form provider-specific extras. Adapters parse what they
    /// understand and ignore the rest. First-class fields above win over
    /// keys here when both are present.
    #[serde(default)]
    pub extra: serde_json::Value,
    /// Usage already written to `agent_usage_ledger` for this thread, so
    /// an adapter that reads a provider-maintained *lifetime* counter can
    /// avoid re-recording history after a session rebuild.
    ///
    /// Only Codex needs this today (see
    /// [`UsageBaseline`]). Adapters that derive their own deltas from
    /// per-message counters ignore it.
    #[serde(default)]
    pub recorded_usage_baseline: Option<UsageBaseline>,
}

/// Token totals already recorded in the usage ledger for one thread.
///
/// Codex's `thread/tokenUsage/updated` reports a **provider-maintained
/// lifetime total** for the thread, and that total survives a
/// `thread/resume`. The delta bookkeeping that turns it into ledger rows
/// lives in adapter memory, which does *not* survive — a session is
/// rebuilt both across app restarts and mid-lifetime, whenever the
/// child dies and the next send resumes it.
///
/// Without a baseline the first report after a rebuild would see a
/// previous total of zero and re-record the thread's entire history as
/// one delta, double-counting on every resume. Seeding from what the
/// ledger already holds closes that.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageBaseline {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    /// Subset of `output_tokens`. Carried because Codex reports it
    /// cumulatively too, so a baseline without it would let a resumed
    /// thread re-record its whole reasoning history.
    pub reasoning_tokens: u64,
}

impl UsageBaseline {
    pub fn is_zero(&self) -> bool {
        self.input_tokens == 0
            && self.output_tokens == 0
            && self.cache_read_tokens == 0
            && self.cache_write_tokens == 0
            && self.reasoning_tokens == 0
    }
}

/// Parameters for queueing a user turn on an existing session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendTurnInput {
    /// Thread the turn belongs to.
    pub thread_id: ThreadId,
    /// Plain-text content of the user message.
    pub text: String,
    /// Unexpanded text used for user-visible queue/persistence events.
    #[serde(default)]
    pub display_text: Option<String>,
    /// Inline image attachments, in order.
    #[serde(default)]
    pub images: Vec<ImageInput>,
    /// Backend-resolved skill selections. The command layer reconstructs
    /// these from stable ids; callers cannot supply arbitrary paths/bodies.
    #[serde(default)]
    pub skill_invocations: Vec<crate::skills::ResolvedSkillInvocation>,
    /// Optional per-turn model override. Not all providers support this.
    pub model_override: Option<String>,
    /// Optional per-turn effort override. Used by Codex which applies
    /// effort on `turn/start`; Claude ignores this (its effort is baked
    /// into the session).
    #[serde(default)]
    pub effort_override: Option<String>,
    /// Optional per-turn permission-mode override. Used by Codex
    /// (sandboxPolicy on `turn/start`). Claude ignores this — its
    /// permission mode is session-scoped and changes require restart.
    #[serde(default)]
    pub permission_mode_override: Option<String>,
    /// Optional client-generated correlation token for the follow-up
    /// queue. The frontend attaches this when it optimistically appends
    /// a user bubble; if the send is queued behind an active turn the
    /// backend echoes it on [`ProviderRuntimeEvent::TurnQueued`] so the
    /// reducer can grey out the existing bubble rather than duplicate it.
    /// `None` for non-queued sends and older callers.
    #[serde(default)]
    pub client_nonce: Option<String>,
}

// ---------------------------------------------------------------------------
// Chat-side capabilities (UI-facing)
// ---------------------------------------------------------------------------

/// Granularity at which a provider applies an effort / reasoning change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffortGranularity {
    /// Change requires a silent session restart (Claude).
    PerSession,
    /// Change applies to the next turn without restart (Codex).
    PerTurn,
}

/// A single context-window option a model exposes (e.g. 200k / 1M).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextWindowOption {
    /// Machine-identifier used in API calls (e.g. `"200k"`, `"1m"`).
    pub value: String,
    /// Human-readable label (e.g. `"200k"`, `"1M"`).
    pub label: String,
    /// True when this option should be selected by default.
    #[serde(default)]
    pub is_default: bool,
    /// The option's window size in tokens (e.g. `200_000` for `"200k"`).
    ///
    /// Lets the UI seed the context meter's denominator at first paint,
    /// before the provider has reported a runtime figure of its own.
    /// `None` when the registry does not state a number — never
    /// guessed from the label. `#[serde(default)]` so payloads
    /// persisted before this field existed still deserialise.
    #[serde(default)]
    pub context_window_tokens: Option<u64>,
}

/// A single permission mode a provider exposes (e.g. Claude's
/// `default` / `acceptEdits` / `bypassPermissions`; Codex's sandbox
/// policies). Value strings are provider-native — each adapter
/// interprets them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionModeOption {
    /// Machine identifier (provider-native).
    pub value: String,
    /// Human-readable short label (e.g. `"Supervised"`, `"Workspace write"`).
    pub label: String,
    /// One-line description shown under the label in the picker.
    pub description: String,
    /// True when this is the provider's default mode.
    #[serde(default)]
    pub is_default: bool,
}

/// Chat-side model metadata — what the composer pickers need to know.
///
/// Separate from the per-provider runtime capabilities (`capabilities()` on
/// the trait) because this lives in the UI layer and is serialized to the
/// frontend verbatim.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatModelInfo {
    /// Model identifier (e.g. `"claude-opus-4-7"`).
    pub id: String,
    /// Human-readable label.
    pub label: String,
    /// Optional one-line description.
    #[serde(default)]
    pub description: Option<String>,
    /// Effort levels the model honours natively via the SDK/RPC param.
    #[serde(default)]
    pub effort_levels: Vec<String>,
    /// Which effort level should be selected when none is configured.
    #[serde(default)]
    pub default_effort: Option<String>,
    /// Per-effort human descriptions reported by the provider's own
    /// catalog, keyed by effort level. Empty when the provider reports
    /// none (Claude, OpenCode) — the picker then falls back to its
    /// built-in descriptions. Provider-reported text wins so a newly
    /// advertised level reads correctly without a frontend bump.
    #[serde(default)]
    pub effort_descriptions: std::collections::HashMap<String, String>,
    /// Effort levels that the UI implements via a prompt-prepend rather
    /// than the SDK/RPC param. Claude uses this for `ultrathink`.
    #[serde(default)]
    pub prompt_injected_effort_levels: Vec<String>,
    /// Context-window options. Empty = not applicable.
    #[serde(default)]
    pub context_window_options: Vec<ContextWindowOption>,
    /// True when the model supports adaptive thinking (Claude Opus 4.6+).
    #[serde(default)]
    pub supports_adaptive_thinking: bool,
    /// True when the model exposes a thinking on/off toggle but no effort
    /// levels (Claude Haiku). Not rendered in MVP.
    #[serde(default)]
    pub supports_thinking_toggle: bool,
    /// True when the model supports the fast-mode flag.
    #[serde(default)]
    pub supports_fast_mode: bool,
    /// True when the model accepts image attachments (multimodal
    /// input). Drives the `+ → Image…` enable state and whether the
    /// composer's paste/drop handlers stage attachments at all.
    /// Defaults to false so an unmapped model is never silently
    /// surfaced as multimodal — the UI prefers a false-negative chip
    /// to a 400-from-the-API.
    #[serde(default)]
    pub supports_images: bool,
    /// Step 12 Stage 3 — for federated providers (OpenCode), the
    /// upstream provider id this model belongs to (e.g. `"openai"`,
    /// `"anthropic"`, `"openrouter"`). `None` for direct providers
    /// (Claude, Codex) where the driver IS the provider. Drives the
    /// picker's grouping rail and the secondary label rendered
    /// below the model name.
    #[serde(default)]
    pub sub_provider: Option<String>,
    /// True when the model is free-tier on the upstream provider's
    /// configured plan (both input and output token costs are 0 in
    /// the harvest response). Today only OpenCode federated entries
    /// can ever set this — Claude and Codex are paid plans across
    /// the board, so their fallback bundles always set this `false`.
    /// Drives a "FREE" pill in the picker and a soft sort boost so
    /// the free-tier models float to the top of their provider's
    /// list (after favorites).
    #[serde(default)]
    pub is_free: bool,
    /// The model's context-window size in tokens, when the registry
    /// knows it outright (either harvested live from the provider or
    /// stated explicitly in the maintained entry).
    ///
    /// Seeds the context meter's denominator before the provider
    /// reports a runtime figure. Models that expose a
    /// [`ContextWindowOption`] picker leave this `None` and carry the
    /// number on the *selected option* instead, since the effective
    /// window depends on which option is active. `None` is always the
    /// safe answer — the UI degrades to a bare token count rather than
    /// rendering a guessed percentage.
    #[serde(default)]
    pub max_context_tokens: Option<u64>,
}

/// Bundle of chat-side capability data for a single provider. Returned by
/// the `list_chat_provider_capabilities` Tauri command and cached
/// client-side in a Zustand store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderChatCapabilities {
    /// Models the provider exposes, in the order they should be displayed.
    pub models: Vec<ChatModelInfo>,
    /// How the provider applies effort changes.
    pub effort_granularity: EffortGranularity,
    /// Canonical label for each effort value ("xhigh" -> "Extra High",
    /// etc.). Lets the UI render consistent names across models.
    #[serde(default)]
    pub effort_label_map: std::collections::HashMap<String, String>,
    /// Permission / approval modes the provider honours. Empty means
    /// the provider has no concept of permission modes and the UI
    /// should hide the picker.
    #[serde(default)]
    pub permission_modes: Vec<PermissionModeOption>,
    /// Default permission mode value when none has been selected.
    /// Should match one of the `permission_modes` entries marked
    /// `is_default: true`.
    #[serde(default)]
    pub default_permission_mode: Option<String>,
    /// How the provider applies permission-mode changes. Typically
    /// matches `effort_granularity` but kept separate in case a future
    /// provider diverges.
    pub permission_granularity: EffortGranularity,
}

/// Result of a successful [`AgentProvider::send_turn`](super::AgentProvider::send_turn).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnStartResult {
    /// Provider-assigned turn identifier. Subsequent events for this turn
    /// reference it. For a **queued** send (see `queued_id`) this is an
    /// empty placeholder — no live turn exists yet; the real turn id
    /// arrives later on the [`ProviderRuntimeEvent::QueuedTurnDispatched`]
    /// event.
    pub turn_id: TurnId,
    /// Set when the send was **queued** behind an in-flight turn instead
    /// of starting immediately. Identifies the queued item so the UI can
    /// render it greyed-out and cancel it. The turn dispatches (emitting
    /// `QueuedTurnDispatched` then the normal turn-start events) once the
    /// session next goes idle. `None` for a normal immediate start.
    #[serde(default)]
    pub queued_id: Option<String>,
}

/// Outcome of an internal session-level send: either the turn started
/// immediately or it was queued behind the active turn.
#[derive(Debug, Clone)]
pub enum SendOutcome {
    /// The turn started immediately; carries its freshly-minted id.
    Started(TurnId),
    /// The turn was queued behind the active turn; carries the queued id.
    Queued(String),
}

/// Decision the runtime ships back to the provider when a user finishes
/// evaluating a pending approval request.
///
/// Variants intentionally mirror the shapes both providers already expose —
/// Claude's `canUseTool` callback and Codex's `item/*/requestApproval`
/// stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum ApprovalDecision {
    /// Approve the request. `updated_input` may override the tool input the
    /// model provided (e.g. the user edited the command before running).
    /// `updated_permissions` carries opaque SDK-shaped `PermissionUpdate[]`
    /// values that the provider persists (e.g. "always allow Bash" rules).
    /// Left `None` for a one-shot approve.
    Allow {
        updated_input: Option<serde_json::Value>,
        #[serde(default)]
        updated_permissions: Option<Vec<serde_json::Value>>,
    },
    /// Approve and remember this decision for the rest of the session.
    AllowForSession,
    /// Reject the request with a user-visible message passed back to the
    /// model.
    Deny { message: String },
    /// Cancel out of the request entirely, halting the turn.
    Cancel,
}

/// Lifecycle phase of a provider session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SessionStatus {
    /// Start-up is in progress; no turns accepted yet.
    Starting,
    /// Session is idle and ready for a new turn.
    Ready,
    /// A turn is actively running.
    Running { active_turn: TurnId },
    /// Waiting for the user to resolve an approval request.
    WaitingApproval { request_id: RequestId },
    /// Session is broken and cannot accept further turns.
    Error { message: String },
    /// Session has been shut down (gracefully or otherwise).
    Closed,
}

/// Snapshot of a live provider session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSession {
    /// Runtime thread this session is bound to.
    pub thread_id: ThreadId,
    /// Which provider produced it.
    pub provider: ProviderKind,
    /// Provider-internal session identifier.
    pub session_id: ProviderSessionId,
    /// Current lifecycle status.
    pub status: SessionStatus,
    /// Opaque resume cursor suitable for passing back into
    /// [`StartSessionInput::resume_cursor`] after a restart.
    pub resume_cursor: Option<serde_json::Value>,
}
