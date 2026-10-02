# Provider commands and skills in agent chat

The audit found missing adapter support, stale discovery and ambiguous skill
tokens. It also found that chat-mode/context prefixes could turn native commands
into ordinary prompts. T3 Code's provider adapters were compared with Codemux
and the providers' documented transports; advertising a terminal command alone
does not make it executable through an SDK or server.

## Implemented behavior

| Provider | Command discovery and dispatch |
| --- | --- |
| Codex | `/compact` calls `thread/compact/start`. `/review` calls inline `review/start`, supporting uncommitted changes, `branch <name>`, `commit <sha>` and custom instructions. Final review output renders in the transcript. `/hooks` opens a native inventory and trust/enable manager, using `hooks/list` and `config/batchWrite`. |
| Claude Code | Existing SDK discovery and native slash dispatch are retained. Project catalogues now expire after a minute and `/refresh` bypasses the cache. |
| OpenCode | Project-scoped `/command` discovery exposes built-in, custom and MCP commands. Native invocations use `/session/:id/command`; `/compact` uses `/summarize` with the selected model. Session requests and events now carry the worktree directory. |
| Cursor / Grok | Existing ACP discovery and slash dispatch are retained. Each live conversation owns its catalogue, including initialization and idle updates; another session in the same directory cannot replace it. Snapshots refresh on opening command context and every two seconds while its menu remains open. |
| Hermes | `available_commands_update` is recorded while idle and during session creation. Each conversation reads its own runtime/session catalogue; profiles sharing a directory never share commands. |

Type `/` for the combined menu, or `$` for skills only. Existing `/skill`
addresses remain supported. Native commands own matching leading slash names;
the corresponding skill stays accessible as `$skill`. Conflicting skill
definitions retain their qualified provider/scope addresses. Exact skill IDs
still reach backend validation and the provider-specific invocation mechanism.

`/help` opens a searchable catalogue for the selected provider: commands prepare
an invocation, and local controls open their GUI. `/skills`, `/settings` and
`/usage` open the corresponding existing settings pages for all six providers.
Home command discovery resolves against the user's home directory, matching
skill discovery; it no longer silently returns an empty catalogue.
Home also probes the native Codex/OpenCode skill catalogues, which previously
only ran after anchoring a project. Built-in and plugin skills reported by the
provider now appear there too. Failed Codex inventory probes always shut down
their temporary app-server child.

`/refresh` reloads commands and skills. `/mcp` opens the existing connected-tools
menu. `/permissions` opens the existing access picker when the provider and chat
state permit configuration. Existing `/model`, chat modes, workflows and message
delivery controls remain available. Exact command matches precede unrelated
skill-description matches, and long labels truncate rather than wrapping.
Selected dollar aliases retain dollar syntax, including when picked from `/`.
Local composer commands also work through Send after dismissing completion.
Command and skill rows display a single source provider mark. The selected row's detail identifies its type and scope, and
whether a skill is native or portable in the active provider. A Claude skill
used in Codex keeps its Claude source identity. Arrow navigation stops at the
list boundaries, and hover does not change the menu’s scroll position. Held-key
repeats are coalesced per frame and cancelled on release or focus loss. The
catalogue stays mounted while its selected row changes, avoiding whole-list
rendering for every repeat in large skill inventories.

The Codex hook manager lists native definitions, MCP-tool hooks, configuration
warnings and errors. Trust writes the exact reviewed hash; edits to the definition
require refreshing and reviewing again. Enable/disable preserves other hook
state, reloads native user configuration, and refreshes the authoritative result.
Managed hooks remain read-only. Home/drafts probe without creating a thread or
starting inference; live conversations reuse their native app-server child.

Initial native discovery blocks sending an ambiguous command until its catalogue
arrives. Failed discovery can be retried; refresh failures retain the previous
catalogue. Restored and pasted command drafts also trigger discovery. Leading
native commands retain their position when modes/effort are selected; attachment
context follows their arguments. Unsupported arguments/images are rejected by
the relevant native action instead of silently becoming prose.

Native actions use the normal turn/queue lifecycle. OpenCode's synchronous
generation endpoints run asynchronously so output can stream and the composer
stays responsive. Successful actions settle through the final SSE idle event;
late HTTP errors cannot clear a newer turn. Active Codex/OpenCode turns reject
native command steering with a Queue/Interrupt instruction.

## Scope and verification

Terminal-only commands (login, terminal display, editor switching and similar
client actions) are not executable remote agent commands. Their existing GUI
equivalents remain controls rather than invented slash prompts. Future provider
features need an adapter contract; this audit does not certify every terminal
feature, plugin or future CLI version. In particular, this does not implement
every provider's plugin marketplace, terminal hook editor, login flow, or every
Codex terminal action such as pets, voice and terminal display configuration.
Codex has no generic slash-command RPC: additions with real APIs require adapter
support. Claude/OpenCode/ACP catalogues can automatically surface new commands
when those providers advertise them through their existing transport.
Hermes continues to exclude projected
Codemux skills, as documented in [its compatibility restrictions](../hermes-provider.md).

Verification includes TypeScript checking, the affected frontend tests, Rust
checking, focused native-command tests and a fake app-server integration test
that verifies Codex RPC payloads and turn completion. GUI checks use the Tauri
mock and screenshots of native-command and dollar-skill completion. No live
model inference or paid provider requests are required by these checks.

GUI evidence: [native command completion](../screenshots/provider-commands/compact.png)
and [dollar skill completion](../screenshots/provider-commands/skills.png),
[native hook management](../screenshots/provider-commands/hooks.png), and
[provider command browsing](../screenshots/provider-commands/help.png).

Sources: [Codex app-server](https://developers.openai.com/codex/app-server/),
[Codex hooks](https://learn.chatgpt.com/docs/hooks),
[OpenCode server](https://dev.opencode.ai/docs/server/),
[OpenCode session routes](https://github.com/anomalyco/opencode/blob/v1.2.27/packages/opencode/src/server/routes/session.ts),
and [T3 provider adapters](https://github.com/pingdotgg/t3code/tree/5cc99e1c23980d7995a13c47f969b47cb68ed1be/apps/server/src/provider/Layers).
