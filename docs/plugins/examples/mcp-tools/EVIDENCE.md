# Plugin MCP live evidence — Linux, 2026-10-19

A real isolated native Codemux loaded a disk plugin running the committed
`src-tauri/tests/fixtures/plugin_mcp.py`. All tested chats started with zero
fixture tools. Writing `ready` announced list_changed; four tools arrived in two
pages. The native Wayland window stayed on an owned headless output with a
no-focus rule, using isolated XDG data.

Every discovery run used this exact user task, with no tool-name hint:

> Use the project integration to retrieve its private verification word. Do not read files or run shell commands. Return the word.

| Real model condition | Observed result |
|---|---|
| Codex gpt-6-luna / low, bridge alone | 0/1; no tool calls, answered no integration was available. |
| Codex gpt-6-luna / low, bounded next-turn reminder | 3/3 fresh chats; list plugin tools → call fixture__new_tool → ORCHID-742. |
| Codex gpt-6.1-sol / low, bridge alone | 1/1; list → call → ORCHID-742. |
| Codex gpt-6.1-sol / low, reminder | 1/1; list → call → ORCHID-742. |
| Claude, native refresh | ToolSearch for verification word project found mcp__codemux__fixture__new_tool; native call returned ORCHID-742. |

These are acceptance samples, not reliability estimates. Native runs used
codex-cli 0.163.0, Claude Code 2.1.303 and Claude Agent SDK 0.2.114.

A second weak-model turn had no repeated reminder: the canonical rollout held
two task messages, with the marker only on the first. App DB user rows contained
exactly the original texts. Provider user-message echoes are ignored; a focused
rollout-import test strips the marker from canonical and event-mirror forms.
Rehydrate, queue edits, forks and provider handoffs use the original app rows.
Resume metadata retains original native tool names and the announced catalog key.

## Claude swap safety

A real SDK probe reported tools:[before]. Replacing an SDK instance under the
same codemux name returned no errors but still reported before. Removing it
with setMcpServers({}) and re-adding it reported after. The installed SDK connects
a replacement instance only when its name was absent.

The fix coalesces changes until the turn result boundary. In a real native chat,
the fixture announced a fifth tool during fixture__slow. Its result
LONG-CALL-COMPLETED reached Claude after 3,001 ms, followed by its normal answer.
The next turn found fixture__after_call through ToolSearch and called it. Saved
events contained no extra visible init/system item. The sidecar test checks
coalescing, forwarded results, and unchanged model/permission options.

## Protocol, names and Stop

The committed capture-only codex-probe.py exercised codex-cli 0.162.1. Generated
experimental schemas exposed dynamicTools only on thread/start. Resume did not
replace the next inference's tools. HTTP MCP with SSE/session-id opened GET,
but list_changed caused no new tools/list. config/mcpServer/reload returned {}
without updating the running thread's model tools. No user config was written.

Claude's observed mcp__codemux__ envelope adds 14 characters. Tests bound registry
plugin names to50 ASCII characters, including deterministic hash suffixes, so
full names fit64. Current [Claude documentation](https://platform.claude.com/docs/en/agents-and-tools/tool-use/define-tools)
and the installed SDK permit128; [OpenAI's function definition](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)
retains64. The implementation fits the stricter full-name budget in this brief.

The Linux immediate-stop fixture writes every10ms and ignores EOF/SIGTERM.
Observed confirmed exit:1.116ms; side-effect timestamps after request:0.
The graceful variant continued through the existing two-second EOF budget.
Other focused tests cover pagination, notifications, activity pairs,
cancellation, stable disable, private/workspace scope, startup disposal and
matching Claude/Codex description payloads. Windows/macOS were not live-tested.

## Unity and harness

unity mcp --help confirmed stdio and --project-path. Unity CLI1.0.0-beta.13
initialize advertised tools.listChanged:true and tools/list returned zero.
No notification arrived in five seconds without an Editor. unity license status
--json reported active:false, signedIn:false, licenses:[]. Editor-backed calls
remain pending a licensed connected Editor.

Structured evidence, transcripts, mock Settings before/after PNGs and logs live
under ~/.cache/codemux-agents/proof/codemux-plugin-proof/ (especially evidence/).
Shared launcher/README: ~/.cache/codemux-agents/unity-cua/harness/.
The launcher uses bwrap --dev /dev; without it WebKit could not read urandom.
No discovery-home app-code hook was added. Temporary tracing used a harness
sidecar entry; the ordinary sidecar was rebuilt afterwards. Initial credential
copies were deleted before expiration; later runs RW-bind-mounted originals.
Both real login status checks passed afterwards. Owned sessions, children,
dev servers and headless output were stopped. No X server was started; the
real X0 socket and xdpyinfo -display :0 were healthy after cleanup.

## Focused verification

Observed passing: npm run check; 39 affected frontend tests; sidecar typecheck
and55 session tests; cargo check -j2; 17 plugin_mcp Rust tests and2 existing
remote-session exclusion tests. Every cargo command used the shared cargo-slot
wrapper. Tests were limited to affected filters; no repository-wide suite ran.
