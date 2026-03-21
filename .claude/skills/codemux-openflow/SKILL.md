---
name: codemux-openflow
description: Use when working on OpenFlow orchestration logic — the runtime loop, agent lifecycle, comm log protocol, phase transitions, stuck detection, ASSIGN protocol, model compatibility, wrapper scripts, intervention handling, or the orchestrator prompt. Also use when debugging orchestration behavior or adding new agent adapters.
---

# Codemux OpenFlow Runtime Standards

Patterns and rules for working on the OpenFlow orchestration engine. This file covers the BACKEND runtime, not the UI (use `/codemux-ui` for visual work on OpenFlow components).

For current OpenFlow state, read `docs/features/openflow.md`.
For active work priorities, read `docs/plans/openflow.md`.
For project architecture, read `docs/reference/ARCHITECTURE.md`.

## Ground Rules

1. **Read the current docs first.** OpenFlow changes fast. Always read `docs/features/openflow.md` and `docs/plans/openflow.md` before making changes — they reflect what actually works today.

2. **The backend drives orchestration.** A tokio background task runs the orchestration cycle. The frontend observes via events. Never move orchestration logic to the frontend.

3. **Comm log is the protocol.** Agents communicate through the shared comm log. All phase decisions are based on parsing the comm log. There is no side-channel.

4. **Test with real agents.** OpenFlow logic cannot be verified by unit tests alone. After changes, test with real agent runs using at least 2-3 agents.

5. **Run verification.** `cargo test --manifest-path src-tauri/Cargo.toml` for backend, `npm run verify` for full pass.

---

## Architecture Boundary

OpenFlow is integrated into Codemux but maintains a separate runtime boundary:

- Runtime logic lives under `src-tauri/src/openflow/`
- Orchestration state lives in `OpenFlowRuntimeStore` and `AgentSessionStore`
- OpenFlow workspaces are runtime-oriented surfaces, not long-term persisted workspace state
- The runtime should stay modular enough to extract and embed elsewhere later

Do not entangle OpenFlow runtime logic with general workspace or pane management code.

---

## The ASSIGN Protocol

This is the core communication mechanism. The orchestrator delegates work to agents by writing ASSIGN lines to the comm log.

**Format:** `ASSIGN <agent-name>: <task description>`

Rules:
- Only the orchestrator writes ASSIGN lines
- Phase transitions key off literal `ASSIGN ...` lines, not vague prose about assigning work
- Each ASSIGN should name a specific agent and give a clear, actionable task
- The orchestrator should assign to available workers, not to itself

**Auto-translator:** When models use internal delegation (like opencode's "General Agent" / "Explore Agent" pattern) instead of ASSIGN lines, the system auto-detects and converts to proper ASSIGN assignments. This is a compatibility shim, not the preferred path — native ASSIGN output is always more reliable.

---

## Phase Transitions

Phases are determined by `determine_next_phase` based on comm log analysis. There is ONE phase system — do not create parallel phase logic.

Key rules:
- **Only explicit `DONE:` markers from agents trigger phase advancement.** Error output, log noise, and ambiguous text must never falsely mark agents as done.
- **Implicit completion is removed.** An agent is not done until it says `DONE:`.
- **WaitingApproval stays until explicit user action.** The run stays in WaitingApproval until the user approves or rejects. It does not auto-complete on the next cycle.
- **Blocked phase is recoverable.** User messages can transition a Blocked run back to Replanning.
- **Completed runs can re-enter replanning** on follow-up user messages.

---

## Stuck Detection

The orchestration loop monitors for stalled agents and runs probe/rescue cycles.

Thresholds:
- Probe at ~50 seconds of no progress
- Rescue at ~60 seconds (active phase) / ~90 seconds (planning phase)
- These thresholds are intentionally high to give agents time to work

Rules:
- Probes are written directly to the comm log as `[SYSTEM] PROBE:` entries — NOT injected via PTY shell escaping
- Probe re-arming clears on meaningful progress, not just counts
- Planning phase gets longer thresholds because thinking takes time
- Do not lower thresholds without testing on real multi-agent runs

---

## Agent Wrapper Lifecycle

Agent wrappers stay alive after the first command so the orchestrator and workers can accept later prompts within the same run.

Rules:
- Wrapper script uses `eval` instead of `bash -lc` to avoid quoting issues
- Wrapper gracefully handles command failures instead of crashing
- Stray GUI launches from agent sessions must be prevented
- `--system-prompt` must be passed on EVERY `claude -p` call including `--resume` calls (it does not persist across sessions)

---

## User Injection Handling

Users can inject messages to the orchestrator via the communication panel.

Rules:
- Injections wake the orchestration loop immediately via `tokio::sync::Notify`
- Injection handling waits for an actual orchestrator response before a user message is considered handled
- Handled injections are tracked via `HANDLED_INJECTIONS:` system messages in the comm log
- Missing orchestrator PTYs are respawned when possible to handle injections

---

## Comm Log Management

The comm log grows during long runs and needs rotation.

Rules:
- Rotation preserves the run header lines (`GOAL`, `APP_URL`, `AGENTS`) so long runs keep core context
- System markers (`HANDLED_INJECTIONS:`, `HANDLED_ASSIGNMENTS:`, `DONE_RELAY_COUNT:`, `INJECTION_PENDING:`) are filtered from UI display but remain in the raw log
- The frontend limits display to the last 100 messages for performance

---

## Model Compatibility

Not all models work equally well as orchestrators.

| Model | Orchestrator Quality | Notes |
|-------|---------------------|-------|
| Claude (any model via Claude CLI) | Excellent | All models follow ASSIGN protocol reliably. Tested at 100% compliance. Recommended for orchestrator role. |
| opencode + Claude-based models | Good | Generally reliable |
| opencode + MiniMax-M2.7 | Unreliable | ~33% ASSIGN compliance. Internal delegation competes with ASSIGN protocol. Auto-translator mitigates but native ASSIGN is better. |

When adding new model adapters:
- Test ASSIGN protocol compliance with at least 5 runs
- If the model uses internal delegation patterns, add auto-translator support
- Document compliance rate in `docs/features/openflow.md`

---

## Adding New Agent Adapters

When adding support for a new CLI agent (beyond Claude CLI and opencode):

1. Create an adapter struct implementing the agent interface
2. Create a dedicated wrapper script if needed
3. Handle the agent's specific CLI flags for: non-interactive mode, system prompt injection, session continuation, permission bypass
4. Test that the agent can: receive tasks via comm log, execute work, and signal `DONE:` when finished
5. Verify the agent works as both orchestrator and worker
6. Document in `docs/features/openflow.md` under Model Compatibility

---

## OpenFlow Is Project-Agnostic

OpenFlow must work for any repo, any language, any toolchain. Web apps are only one test case.

Rules:
- Orchestration, delegation, and stuck detection must not assume web development
- Browser/live-preview hints are optional, not required
- The `APP_URL` feature is for convenience, not a hard dependency
- Test OpenFlow with non-web projects (CLI tools, libraries, data pipelines) to verify generality

---

## Do Not

- Create parallel phase systems — all phase logic goes through `determine_next_phase`
- Move orchestration logic to the frontend — the backend tokio task drives everything
- Lower stuck detection thresholds without testing on real runs
- Inject probes via PTY shell escaping — write directly to comm log
- Assume a specific model or CLI tool — keep the adapter pattern
- Let error output or log noise trigger phase advancement — only explicit `DONE:` markers count
- Mix OpenFlow runtime logic with general workspace/pane management code
