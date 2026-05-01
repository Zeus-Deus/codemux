//! OpenCode [`AgentProvider`](crate::agent_provider::AgentProvider)
//! integration — Step 12 Stage 1 scaffold.
//!
//! OpenCode breaks the JSON-RPC-stdio pattern that the Claude and Codex
//! adapters share. The `opencode` CLI ships a long-running local HTTP
//! server (`opencode serve --hostname=127.0.0.1 --port=<auto>`) that
//! speaks v2 of `@opencode-ai/sdk` over plain HTTP, so the runtime
//! adapter is built around `reqwest` instead of [`json_rpc_child`]. This
//! module is the seam where that work lands.
//!
//! # Stage 1 scope
//!
//! What lands here right now:
//!
//! * Binary discovery — locate `opencode` on PATH, probe `--version`,
//!   and surface a structured availability result. Never panics on a
//!   missing binary; that's a normal "not installed" signal that the UI
//!   later in the project will render as an empty-state.
//! * HTTP client scaffolding — a `reqwest::Client`-backed wrapper with a
//!   `ping()` and a `list_models()` helper plus the response shapes the
//!   live harvest in Stage 2 will populate. No request is wired into
//!   the production runtime path yet; the surface exists so Stage 2 can
//!   land without churning the public module shape.
//! * Per-instance shim — see [`instance`](crate::agent_provider::instance)
//!   for the `instance_id === driver` forward-compat hook used by every
//!   serialized payload that Stage 1 introduces.
//!
//! # What is NOT here yet
//!
//! * No `AgentProvider` impl. The trait surface (`start_session`,
//!   `send_turn`, `event_stream`, etc.) needs an HTTP-server lifecycle
//!   plus an async-iterable event-subscription bridge that takes real
//!   design work — that's Stage 2/3 territory.
//! * No spawn lifecycle. Discovery does NOT start `opencode serve`.
//!   Stage 2 wires the spawn + stdout-line URL detection
//!   (`"opencode server listening"` prefix from the reference clone,
//!   verified at `/tmp/<reference>/apps/server/src/provider/opencodeRuntime.ts:36`)
//!   on top of the discovery surface here.
//! * No upstream-credential UX. OpenCode reads OpenAI / Anthropic /
//!   Google API keys from `~/.config/opencode/`; Codemux deliberately
//!   stays out of that. See `docs/plans/step-12-opencode-implementation-plan.md`
//!   §2.3 for the rationale.
//!
//! Stage 1 ships nothing user-visible — its job is to extend the schema
//! seam (`ProviderKind::OpenCode`) and stand up the discovery surface so
//! later stages can build the live integration without rewriting prior
//! work.

pub mod capabilities;
pub mod client;
pub mod discovery;
pub mod manager;
pub mod server;

pub use capabilities::opencode_stage1_placeholder;
pub use client::{OpenCodeClient, OpenCodeClientConfig, OpenCodeModel, OpenCodeProviderEntry};
pub use discovery::{check_opencode_availability, OpenCodeAvailability, MINIMUM_OPENCODE_VERSION};
pub use manager::{OpenCodeServerHandle, OpenCodeServerManager};
pub use server::OpenCodeServer;
