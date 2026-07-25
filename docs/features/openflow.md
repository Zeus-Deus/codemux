# OpenFlow Capability

> ## ⚠️ OpenFlow is currently unreachable from a cold start
>
> The sidebar rewrite (PR #198, and the earlier `c11a3fc` that deleted the
> sidebar OpenFlow section) removed the only trigger for the New Run dialog.
> The dialog itself is still mounted globally — `src/components/layout/app-sidebar.tsx`
> renders `<NewRunDialog />` with a comment noting its trigger "used to live inside
> the (now removed) sidebar section" — but the only two callers of
> `setNewRunDialogOpen(true)` are `openflow-workspace.tsx` and
> `orchestration-header.tsx`, both of which render **only when an `open_flow`
> workspace is already active**. `createOpenflowWorkspace` has exactly one caller:
> the dialog itself.
>
> So you need an OpenFlow workspace to open the dialog that creates an OpenFlow
> workspace. There is no CLI, control-socket, or MCP escape hatch, and OpenFlow
> workspaces are deliberately never persisted (`state_impl.rs`: "Never persist
> OpenFlow workspaces or their terminal sessions"), so any surviving run is gone
> after an app restart — making the deadlock permanent.
>
> Everything below describes a runtime that still works once a run exists; it is
> the entry point that is missing. Re-homing one trigger (command palette entry,
> `+` launcher item, or footer nav) restores the whole feature.

- Purpose: Describe what OpenFlow is in Codemux and what currently works.
- Audience: Anyone working on orchestration, agent workflows, or OpenFlow UX.
- Authority: Canonical OpenFlow capability and constraints document.
- Update when: OpenFlow behavior, scope, or reliability expectations change.
- Read next: `docs/plans/openflow.md`, `docs/core/STATUS.md`

## What OpenFlow Is

OpenFlow is Codemux's multi-agent orchestration layer. It should behave like a first-class workspace or run type inside Codemux while keeping a modular runtime boundary that could be reused elsewhere later.

It must remain project-agnostic: web apps are only one test case, not the product boundary. Orchestration, follow-up handling, stuck detection, and delegation rules should work for any repo and any language/toolchain.

## What Works Today

- OpenFlow workspace and orchestration UI shell exist
- real agent PTYs can be spawned and monitored
- run records, phases, retry and cancel style controls, and shared communication logs exist
- sidebar visibility and orchestration monitoring exist in prototype form
- test agents can use Codemux browser automation
- extra diagnostics exist for wrapper lifecycle, native launch attribution, and OpenFlow breadcrumbs
- frontend OpenFlow state and orchestration helpers are now split into dedicated store/modules instead of one catch-all app-state file
- new runs get a run-scoped `APP_URL` for the **orchestrated project's** live preview (default: first free port in 3900–4199), not the Codemux UI; set `CODEMUX_OPENFLOW_APP_URL` when the OpenFlow run should target the shell dev server (e.g. `http://localhost:1420` while developing Codemux)
- agent wrappers stay alive after the first `opencode run`, so orchestrator and workers can accept later prompts inside the same run
- user injections from the communication panel now wake orchestration immediately, completed runs can re-enter replanning on follow-up messages, and missing orchestrator PTYs are respawned when possible
- injection handling now waits for an actual orchestrator response before a user message is considered handled
- orchestrator phase transitions now key off literal `ASSIGN ...` lines instead of vague prose about assigning work
- comm-log rotation now preserves the run header lines (`GOAL`, `APP_URL`, `AGENTS`) so long runs keep their core context
- pausing a run now keeps the run and workspace alive instead of tearing them down immediately
- **orchestration is now backend-driven**: a tokio background task runs the orchestration cycle every 5 seconds (15s when completed/blocked), replacing the old frontend-polling model; user injections wake the loop immediately via `tokio::sync::Notify`
- **stuck probes are now written directly to the comm log** as `[SYSTEM] PROBE:` entries instead of being injected via PTY shell escaping, eliminating the feedback loop where bash errors from broken probes counted as "progress"
- **the daemon-backed agent spawn path now tees the comm log** (`v0.7.9`): persistent agents made daemon-backed spawn the default, but that reader path only drained the PTY mpsc to the output queue and never wrote the communication log — so OpenFlow agents spawned that way produced an **empty** comm log and the orchestrator's stuck-detection / progress analysis went blind. Both reader paths (in-process and `terminal::daemon_backed`) now share the `comm_log_entry_for_chunk(role, data)` helper (clean ANSI, drop spinner/block-glyph/too-short noise, format the tagged timestamped line) and tee each chunk to `$CODEMUX_COMMUNICATION_LOG`, flushed per chunk so the log stays live for the orchestrator. See `docs/features/persistent-agents.md`.
- **Blocked phase is now recoverable**: user messages can transition a Blocked run back to Replanning
- **WaitingApproval now actually waits**: the run stays in WaitingApproval until the user explicitly approves, instead of auto-completing on the next cycle
- **implicit completion removed**: only explicit `DONE:` markers from agents trigger phase advancement; error output and log noise no longer falsely mark agents as done
- **stuck detection thresholds**: probe at ~120s (`STUCK_PROBE_MIN_CYCLES = 24` at the 5s loop interval), rescue at ~60s active (`ACTIVE_STUCK_RESCUE_CYCLES = 12`) / ~90s planning (`PLANNING_STUCK_RESCUE_CYCLES = 18`), giving agents time to work
- **wrapper script hardened**: the opencode wrapper invokes `opencode run "$message" --model "$MODEL"` directly and assembles follow-ups with a bash array rather than a re-quoted shell string (neither `eval` nor `bash -lc` is used), and gracefully handles command failures instead of crashing
- **conflicting phase systems unified**: the `run_autonomous_loop` / `advance_run_phase` auto-advance path has been removed; all phase logic goes through `determine_next_phase` driven by comm log analysis

- **Claude Code CLI adapter implemented**: full adapter with dedicated wrapper script supporting `claude -p` with `--system-prompt`, `--resume` for session continuation, `--output-format json` for session ID capture, and `--permission-mode bypassPermissions`; works with any Claude model (haiku, sonnet, opus)
- **auto-translator for internal delegation**: when models use opencode's internal "General Agent" / "Explore Agent" delegation instead of ASSIGN lines, the system auto-detects the pattern and converts it to proper ASSIGN assignments for available workers

## What Is Still Prototype-Level

- browser view inside the OpenFlow workspace is still not the final integrated experience
- pause/resume semantics are better than before but still need a cleaner explicit suspended-state model
- dev-time lifecycle issues and single-instance hardening still need work

## Model Compatibility

- **Claude models (via Claude CLI)**: highly recommended for orchestrator role; all models (haiku, sonnet, opus) follow the ASSIGN protocol reliably; tested at 100% compliance
- **opencode with Claude-based models** (e.g., github-copilot/claude-sonnet-4.6): generally reliable
- **opencode with MiniMax-M2.7**: limited as orchestrator (~33% ASSIGN compliance); the model's internal delegation system competes with OpenFlow's ASSIGN protocol. Works better with auto-translator enabled

## Constraints

- Codemux is the primary host experience, but the runtime should stay modular
- the OpenFlow browser view currently mounts the shared default browser session rather than a run-scoped embedded browser surface
- `--system-prompt` must be passed on EVERY `claude -p` call including `--resume` calls (it does not persist across sessions)
