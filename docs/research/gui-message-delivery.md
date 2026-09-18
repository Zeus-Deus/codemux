# GUI message delivery

Investigated and implemented on 2026-09-18. These controls apply to ordinary
agent chat as well as goal runs; they do not require a goal.

| Command | While working | When idle |
| --- | --- | --- |
| `/queue text` (default) | FIFO follow-up after the current turn | Start a turn |
| `/steer text` | Native guidance in the existing turn, where supported | Start a turn |
| `/interrupt text` | Explicitly interrupt, then dispatch this follow-up first | Start a turn |

The busy composer exposes these choices beside the send and Stop buttons.
Queued bubbles expose Steer (where supported), Interrupt, and Cancel. Commands
are local controls: the leading prefix is removed before skill expansion or
provider submission. Quoted commands, paths, and commands inside a message
remain literal text. Failed submissions restore the command and retain the
attachments. Failed promotion retains the queued message.

## Provider findings

| Provider | Safe steering | Evidence |
| --- | --- | --- |
| Codex | Yes: `turn/steer` with `expectedTurnId` | Installed CLI 0.154.0; native source and app-server protocol; adapter integration tests |
| OpenCode | Yes: active-session `prompt_async` | Installed 1.18.31; version-pinned source; live deterministic local-model probe |
| Claude | Unavailable | SDK 0.2.114 and CLI 2.1.272; live gated MCP-tool experiment |
| Cursor / Grok | Unavailable | ACP prompt/cancel protocol; no verified non-interrupting delivery operation in the adapters |

Unsupported steering is visibly disabled while busy and rejected by the
backend. It never silently becomes an interrupt. Queue and explicit interruption
remain available with the same names for every provider. Supporting a new native
steering mechanism requires adapter implementation and a capability change.

### Codex

[App-server steering documentation](https://learn.chatgpt.com/docs/app-server#steer-an-active-turn)
and local `codex-rs/core/src/session/mod.rs::steer_input` confirm that input is
queued inside the active turn, without aborting tool execution. Existing CodeMux
asynchronous-question delivery already uses this native operation.

The implementation reads native turn state, binds guidance to its ID, and only
retries an explicit precondition rejection after refreshing state. A turn that
completed during submission becomes an ordinary follow-up. Ambiguous transport
failures are surfaced, not automatically resent. Review/compaction or an older
server may reject native guidance; the draft is retained.

### OpenCode

Use the installed **v1** protocol, not the newer v2 documentation:

- [v1.18.31 session prompt](https://github.com/anomalyco/opencode/blob/v1.18.31/packages/opencode/src/session/prompt.ts)
- [v1.18.31 runner](https://github.com/anomalyco/opencode/blob/v1.18.31/packages/opencode/src/effect/runner.ts)
- [v1.18.31 run state](https://github.com/anomalyco/opencode/blob/v1.18.31/packages/opencode/src/session/run-state.ts)

The prompt is persisted before `ensureRunning` joins the current run. Its next
iteration reads the new message. The live probe supplied a local deterministic
OpenAI-compatible endpoint, ran a harmless three-second shell tool, submitted a
correction while the tool was running, and inspected the next model request.
The tool completed normally; the request contained both `TOOL_COMPLETED` and
`STEER_PROBE_CORRECTION`. Two main model requests were observed, with no abort.
No external model service or credentials were used for this probe. Reproduce it
with `OPENCODE_BIN=/absolute/path/to/opencode python3 scripts/probes/opencode-steering.py`.
Use the actual executable rather than a version-manager shim because the probe
isolates XDG directories.

CodeMux now owns a FIFO queue for OpenCode. It waits for `session.idle`, ignoring
the preceding duplicate `session.status(idle)` boundary. State is committed
before the completion batch is published, so draining cannot complete the next
turn with an old idle event.

### Claude

[SDK streaming input](https://code.claude.com/docs/en/agent-sdk/streaming-vs-single-mode)
is not enough evidence that input safely steers a running tool. The SDK's
`SDKUserMessage.priority` has `now`, `next`, and `later` modes. A real Haiku probe
used an in-process MCP tool held for five seconds, and injected a correction
from inside that tool:

- `next`: the tool completed and the original answer was returned before the
  correction was consumed. This is a follow-up, not active-turn steering.
- `now`: the query ended before the held tool completed. It interrupts.

Neither tested mode provides the promised safe behavior, so this implementation
does not present them as Steer. The live probe used the installed authentication,
an isolated working directory, no persisted session, and only the harmless MCP
tool.

### ACP (Cursor and Grok)

[ACP prompt turns](https://agentclientprotocol.com/protocol/v1/prompt-turn)
provide `session/prompt` and `session/cancel`. The CodeMux adapters already
queue follow-ups and use cancellation for explicit interruption. No safe
in-flight injection mechanism was verified. Installed versions examined:
Cursor 2026.09.10-fd3934a, Grok 1.0.30.

### Hermes reference

Inspected the local Hermes agent and web UI projects. `agent/interrupt_control.py`
stores pending guidance; `agent/agent_runtime_helpers.py` applies it after the
tool batch as a standalone user message. The web UI separates queue, steer,
and interrupt and retains unsuccessful drafts. CodeMux follows that distinction,
using native provider boundaries instead of guessing from UI tool events.

## Persistence and concurrency

Guidance persists as a user envelope with `steered_turn_id`; live fan-out and
history replay both retain that marker. It does not create a new turn boundary.
Attachments and client nonces are registered before dispatch and transferred
atomically to deferred queue persistence. Outbound locks serialize queue
promotion, cancellation, native guidance, and ordinary dispatch.

Pending queues remain process-local, as before; restarting a provider discards
its pending queue. “Accepted” means the native provider accepted the guidance,
not that a particular sentence has already been read by the model.

## Validation

Targeted tests cover command parsing, composer controls, command stripping,
failed-draft restoration, transcript replay, same-turn Codex guidance, completion
races, OpenCode FIFO dispatch and failed queued guidance. Browser verification
uses the Tauri mock at `http://localhost:1420`; it verifies the actual composer,
slash controls, queued actions, and unchanged running tool during mock steering.
Live provider timing evidence is described above; GUI mock tests alone are not
evidence of native tool safety.

Verification results for this change:

- `npm run check`: passed.
- Seven affected frontend test files: 402 tests passed across the final targeted runs.
- `cargo check -j 2 --manifest-path src-tauri/Cargo.toml`: passed.
- OpenCode module tests: 184 passed, one existing ignored test.
- Codex adapter integration tests: 50 passed, one existing ignored test.
- Backend user-message fan-out tests: two passed, including steering metadata on both attached clients.
- Saved OpenCode live probe: passed (completed tool and correction in the next request).
- GUI mock: delivery picker, direct steering, queueing, queued steering, and explicit interruption verified; screenshots captured.

The whole repository suite was not run. These results cover the changed GUI,
transcript handling, and provider adapters. The native Claude probe establishes
why its safe-steering capability remains disabled; it is not a test of a new
Claude delivery implementation.
