# Writing Codemux plugins

A plugin is one ES module that the Codemux window loads into its own
renderer. It can add palette commands, shortcuts, right-panel panes, pages,
settings pages, composer actions, title-bar buttons, sidebar sections, landing
art, tool-call renderers, themes and overlays, and it can read chat events from
every thread.

Plugins are **not sandboxed**. A plugin can do anything the app can: read your
projects, run commands through the app's IPC, talk to the network. Safety comes
from the rails around it, not from isolation:

- **On/off switch:** each plugin has one in Settings → Plugins.
- **Label:** every plugin is labelled Official, Unverified or Local.
- **Install warning:** shown before anything is written.
- **Error isolation:** a crashing contribution shows a "Plugin X failed" chip; the rest of the app keeps working.
- **Kill switch:** `CODEMUX_DISABLE_PLUGINS=1` or `codemux --disable-plugins` loads none.
- **Safe mode:** if the app crashed while loading plugins, the next launch pauses every installed plugin until you resume.

Install only plugins you trust.

## Quick start

1. Open Settings → Plugins → **Open plugins folder**. The folder is
   `<data dir>/codemux/plugins` (`~/.local/share/codemux/plugins` on Linux,
   `%APPDATA%\codemux\plugins` on Windows).
2. Make a folder whose name is the plugin id, e.g. `hello/`, and put a
   `plugin.js` in it. You can start by copying
   [`examples/plugins/hello/plugin.js`](../../examples/plugins/hello/plugin.js).
3. The plugin appears as **Local** and is turned on. Save the file and it
   hot-reloads. **Reload plugins** rescans the folder.

```js
// hello/plugin.js — plain ESM, no JSX
import { host, PALETTE_AREA } from "@codemux/sdk";

export default {
  id: "hello", // must equal the folder name
  name: "Hello",
  description: "Says hello",
  version: "0.1.0",
  register(ctx) {
    ctx.register({
      id: "say",
      area: PALETTE_AREA,
      data: { label: "Hello: say hello", run: () => host.notify({ message: "Hello" }) },
    });
  },
};
```

## The contract

`plugin.js` default-exports a `CodemuxPlugin`:

| Field | |
|---|---|
| `id` | `[a-z0-9-]{1,64}`, equal to the folder name. |
| `name`, `description`, `version` | Shown in Settings → Plugins. |
| `minCodemux` | Refuse to load on an older app, e.g. `"0.25.0"`. |
| `register(ctx)` | Called when the plugin turns on. May be async. A throw or rejection rolls back everything it registered. |

`ctx` (`PluginContext`):

| Member | |
|---|---|
| `ctx.register(contribution)` / `ctx.registerMany([...])` | Add contributions; returns a disposer. Ids are local to your plugin. |
| `ctx.registerSettingsPage({ title, render })` | A page under Settings → PLUGIN SETTINGS. |
| `ctx.onDispose(fn)` | Runs when the plugin turns off, reloads or is removed. |
| `ctx.setTimeout`, `ctx.setInterval`, `ctx.addEventListener` | Like the globals, but cleaned up for you on unload. |
| `ctx.storage.get(key, fallback)` / `.set(key, value)` / `.remove(key)` | JSON in the device-local settings DB under `plugin.<id>.<key>`. Never synced. |
| `ctx.host` | The host API bound to this plugin. Use it for subscriptions made after an `await` or in your own async callbacks; they are removed when the plugin turns off. |
| `ctx.id`, `ctx.source` (`plugin:<id>`), `ctx.tier` | Who you are. |

A contribution is `{ id, area, title?, order?, when?, enabled?, render?, data? }`.

- `render` is mounted as a React component inside its own error boundary.
- `data` is read by data areas: commands, shortcuts, themes, tool renderers.
- `when()` is not reactive. It is evaluated only when the area changes; for visibility that follows app state, return `null` from `render`.

Turning a plugin off removes the entry first, then runs every disposer (each in
its own try/catch). Nothing needs a reload.

## Areas

Import the constants from `@codemux/sdk`.

| Constant | Where | Reads |
|---|---|---|
| `PALETTE_AREA` | Command palette, under "Plugins" | `data: { label, icon?, keywords?, requiresWorkspace?, run({ workspaceId }) }` |
| `KEYBINDS_AREA` | Global shortcuts, listed and rebindable in Settings → Shortcuts | `data: { label, defaultKeys: "Ctrl+Alt+H", category?, description?, run(event) }` (return `false` to let the key through) |
| `RIGHT_PANEL_AREA` | A right-panel pane, offered in the panel's "+" menu under PLUGINS | `title`, `data: { icon? }`, `render({ workspaceId })` |
| `PAGES_AREA` | A full page, opened with `host.openPage("<id>:<localId>")` | `title`, `render()` |
| `APPEARANCE_EXTRA_AREA` | End of Settings → Appearance | `render()` |
| `COMPOSER_ACTIONS_AREA` | Rows in the composer "+" menu | `data: { label, description?, icon?, run({ workspaceId, threadId, composer }) }` |
| `COMPOSER_ACCESSORY_AREA` | Above the composer footer | `render({ workspaceId, threadId })` |
| `COMPOSER_FOOTER_AREA` | Below the composer card | `render({ workspaceId, threadId })` |
| `TITLEBAR_ACTIONS_AREA` | The title bar's action island | `render()` (stop `pointerdown` on controls so the window doesn't drag) |
| `SIDEBAR_SECTION_AREA` | Between the workspace list and the sidebar footer | `render()` |
| `SIDEBAR_EMPTY_STATE_AREA` | Replaces the empty-sidebar plate (first one wins) | `render()` |
| `LANDING_BACKDROP_AREA` | Behind the new-thread landing, `pointer-events: none` | `render({ onPulse, calmRect, onCalmChange })` |
| `LANDING_BELOW_COMPOSER_AREA` | Under the composer on the new-thread landing | `render({ workspaceId })` |
| `TOOL_RENDERER_AREA` | Replaces the core card for a tool call (first match wins) | `data: { tool: "Bash", render({ call }) }` |
| `THEMES_AREA` | Listed in the theme picker; never auto-selected | `data: ThemeDefinition` |
| `APP_OVERLAY_AREA` | Above the whole app: floating UI or headless components | `render()` |

Settings pages use `ctx.registerSettingsPage`, not an area.

## Host API

`import { host } from "@codemux/sdk"`, or `ctx.host`. Every subscription returns an unsubscribe.

**Cleanup when the plugin turns off:** subscriptions made through `ctx.host` are always removed. A bare `host.*` subscription is removed only if it was made synchronously inside `register()` or inside a callback the host invoked for you. After an `await`, the host can't tell which plugin is calling, so use `ctx.host` there.

**Errors in callbacks:** a subscription callback that throws is logged against your plugin. It never breaks the app or other listeners.

| Member | |
|---|---|
| `host.theme.get()`, `host.theme.subscribe(fn)`, `host.theme.cssVar(role)` | The active theme and its CSS variable names. |
| `host.reducedMotion()`, `host.onReducedMotionChange(fn)` | `true` when the app's reduce-motion setting **or** the OS `prefers-reduced-motion` is on. Respect it. `onReducedMotionChange` calls `fn(reduced)` whenever either source flips the result. |
| `host.notify({ kind?, message, description? })` | A toast. It is not stamped with your plugin's name, so say who you are in the message. (Load errors, render crashes and failed commands are reported by the app, named after your plugin.) |
| `host.openPage(id)`, `host.openSettings(section?)`, `host.openRightPane(id)` | Navigation. Ids are full contribution ids, `"<pluginId>:<localId>"`. |
| `host.workspace.current()`, `host.workspace.subscribe(fn)` | The active workspace (`workspace_id`, name, path, …). |
| `host.settings.get(key)`, `host.settings.subscribe(key, fn)` | App settings, read-only. |
| `host.composer.insertText(text)`, `.getDraft()`, `.focus()` | The focused composer (or the only one). Return `false`/`null` when none is mounted. |
| `host.chat.onEvent(fn)` | Every thread's provider events, including background threads with no pane open. `usage_recorded` carries `cacheWriteTtl` (`"1h"` / `"5m"`, Claude only) and `subagent`. The native tap runs only while someone listens. |
| `host.chat.getThread(id)`, `host.chat.listThreads()`, `host.chat.sendTurn(id, text)` | Read a thread; send a turn the same way the send button does. |
| `host.chat.getStatus(id)` | What the thread's live session is doing now: `{ provider, busy, queued, compacting, status }`, or `null` when it has no live session (never started, closed, or the app restarted). `busy` means a turn, a pending approval, a queued send being dispatched or a compaction is in flight. `queued` is how many sends wait behind it, or `null` for providers that keep no queue of their own (only Claude and Codex do). `status` is `starting`, `ready`, `running`, `waiting_approval`, `error` or `closed`. |
| `host.chat.requestCompaction(id)` | Ask the app to compact an idle Claude chat now (Claude Code's `/compact`). Resolves `{ started: true }` or `{ started: false, reason }`; a refusal is a normal answer. **Your plugin decides when; the app keeps it safe:** it refuses unless the session is idle, `ready` and has nothing queued (re-checked under the session lock), claims the session so sends made meanwhile queue behind it, hides the compaction's turn and reply (no "Working" state, no bubble), adds a quiet "Compacted while idle" note to the transcript, and after 10 minutes stops a compaction that hasn't finished before running the queued sends (if the stop can't be confirmed, the session restarts and those sends are cancelled). Reasons: `busy`, `queued`, `not_ready`, `closed`, `no_session`, `not_claude`, `unsupported` (a device or workflow chat), `compaction_off` (`DISABLE_COMPACT`, `DISABLE_AUTO_COMPACT` or `autoCompactEnabled: false`), `idle_compaction_off` (`idleCompaction: false` in Claude settings), `failed`. |
| `host.invoke(cmd, args)`, `host.listen(event, fn)` | **Unstable.** Raw app IPC with full authority. Commands and events can change between releases without notice. |

The SDK also exports the app's UI kit, so plugins look native: `Button`,
`Input`, `Textarea`, `Label`, `Switch`, `Checkbox`, `Select*`, `Dialog*`,
`Popover*`, `Tooltip*`, `DropdownMenu*`, `Badge`, `Tabs*`, `ScrollArea`,
`Separator`, `Kbd`, `Spinner` and `cn`. It also exports lucide icons as `icons`,
zustand's `create`, React Query's `useQuery`, `useMutation` and
`useQueryClient`, and `definePlugin` for typed plugins. The current export list
is `src/sdk/index.ts`.

## Imports and bundling

A disk plugin can import exactly four specifiers: `@codemux/sdk`, `react`,
`react/jsx-runtime` and `react/jsx-dev-runtime`. They resolve to the app's own
modules, so there is one React. Any other import fails the load up front with
the specifier named.

`plugin.js` loads as is, so write plain ESM without JSX
(`React.createElement` or `jsx()` calls). Alternatively, bundle with esbuild:

```sh
esbuild src/index.tsx --bundle --format=esm --external:react --external:react/jsx-runtime --external:@codemux/sdk --outfile=plugin.js
```

A plugin's source is capped at 16 MiB.

## Sharing a plugin

- **From GitHub:** put `plugin.js` at the repository root, or in `codemux/`, or in any folder.
  - Users paste `https://github.com/owner/repo` or `https://github.com/owner/repo/tree/<branch>/<folder>` into **Install from GitHub URL…**.
  - The dialog shows the exact commit it will install.
  - **Update…** later installs whatever that branch or tag points at then, after showing the new commit.
- **What an install copies:** one commit is fetched; nothing in the repository runs (no checkout, no hooks, no scripts, no npm). Only the plugin folder's own regular files are copied: up to 64 files and 32 MiB, no sub-folders and no links.
- **Catalog:** the catalog lives in [`catalog/plugins/catalog-v1.json`](../../catalog/plugins/catalog-v1.json), and entries are added by pull request:
  - Each entry is `{ id, name, description, tier, repo, sha, subdir?, minCodemux?, screenshots }`.
  - It pins one commit.
  - The maintainers set `tier`.
  - The app re-reads the catalog once a day. A `blocked` entry (by `id` or `sha`) refuses that plugin at install and disables it at load, showing the reason.
- **Start state:** installed plugins start **off**, unless the user picks "Install and turn on".

## Pitfalls

- **JSX in a disk plugin won't parse.** Use `React.createElement`/`jsx()` or bundle with esbuild.
- **Track everything you start.** A bare `window.setInterval`, `addEventListener` or appended `<style>` survives turning the plugin off and stacks up on every hot reload, because ES modules can't be unloaded. Use `ctx.setTimeout`/`ctx.setInterval`/`ctx.addEventListener` and put anything else in `ctx.onDispose`. Module-scope state is yours to reset.
- **Subscribe through `ctx.host` after an `await`.** A bare `host.chat.onEvent(...)` made after an `await` has no known owner, so it keeps running after the plugin turns off.
- **Module evaluation has a 10 s deadline.** Don't top-level `await` anything that might not settle; do the waiting inside `register()`.
- **Keep `render` identities stable.** Define components once at module scope, not inline in `register`, or React remounts them on every registry change.
- **Use theme variables, not colours.** `var(--background)`, `var(--foreground)`, `var(--muted-foreground)`, or `host.theme.cssVar(role)`. Redraw canvases on `host.theme.subscribe`.
- **Canvas panes resize constantly.** Track the container with a `ResizeObserver` and set the canvas `width`/`height`, not just CSS.
- **Title-bar controls must stop `pointerdown` propagation,** or a click starts a window drag.
- **The chat tap costs nothing until you listen.** Unsubscribe when you don't need events; the native side stops emitting when the last listener leaves.
- **One plugin can't switch another on or off.** Decisions are written only by Settings.
- **Not on the web remote.** Plugins run only in the desktop window; the web client never loads them, and `plugins_*` commands (including the ones behind `host.chat.getStatus` and `requestCompaction`) are refused there.
- **Check `getStatus` before asking for a compaction, but don't rely on it.** The session can change between the two calls; `requestCompaction` re-checks everything itself and answers `busy` or `queued` if the user got there first.

## Bundled plugins

The app itself ships plugins in `src/plugins/<id>/`:

- `meta.ts` is loaded eagerly: id, name, description and `defaultEnabled`.
- `plugin.ts(x)` is a lazy chunk that is fetched only when the plugin is on.
- Bundled code may import only `@codemux/sdk`, `react`, and sibling files in its own folder (`./field`). Assets and anything outside the folder go through the SDK. This is enforced by `src/plugins/import-fence.test.ts`.

`src/plugins/example` uses every area and is off by default. `src/plugins/thread-backdrop` (Official, on by default) is a complete real plugin: an engraving typed out in faint monospace behind a new thread, built on `LANDING_BACKDROP_AREA`, `ctx.host.theme`, `PLATES` and `onCalmChange`.

## Decisions

Choices the spec left open are recorded one per line in
[`DECISIONS.md`](./DECISIONS.md), each with how to revert it.

## MCP servers and live tools

A desktop plugin can register stdio or Streamable HTTP servers. Await registration
before using `ctx.host.mcp`. It installs the registration immediately; the MCP
stdio child starts immediately in the background (not on the first chat or call). Register only workspaces you need. The handshake also runs in the background. Inspect `getStatus` or `onStatus` for readiness.

```ts
const dispose = await ctx.registerMcpServer({
  id: "editor-project-a",             // unique within this plugin
  name: "Unity Editor",              // Settings → MCP label
  toolPrefix: "unity",                // defaults to the plugin id
  scope: { workspaceIds: [workspaceId] }, // or "all", or "plugin" for UI only
  toolDescriptions: { manage_scene: { append: "Use structured Unity tools first." } },
  shutdown: "graceful",               // or "immediate" for input drivers
  transport: "stdio",
  command: "unity", args: ["mcp", "--project-path", projectPath],
  cwd: projectPath,                   // optional absolute path
  env: {},
});
// HTTP alternative: transport: "http", url: "http://localhost:1234/mcp",
// headers: { Authorization: "Bearer …" }.
const stop = ctx.host.mcp.onStatus("editor-project-a", (status) => {
  // status is null after removal; otherwise { id, name, status, toolsCount,
  // errorMessage, stderrTail, startedAtMs }. status.kind is discovered,
  // starting, running, errored, stopping or stopped (running has toolCount).
});
const tools = await ctx.host.mcp.listTools("editor-project-a");
const result = await ctx.host.mcp.callTool("editor-project-a", "screenshot", {});
// result.content includes original text/image/resource blocks; base64 image
// data, mimeType, structuredContent and result extension fields are preserved.
```

Registrations and subscriptions are owned by that loaded plugin incarnation.
Turning it off, removing it or reloading it removes its servers and tools, and
cancels in-flight calls. `await dispose()` is safe to call manually and resolves
only after the stdio child has exited and in-flight calls have ended. Status
moves through `stopping`, then `stopped` only after confirmed exit. Default
`shutdown: "graceful"` closes stdin and permits two seconds before killing the
child. `shutdown: "immediate"` skips that grace: on Linux and macOS it immediately kills
the owned process group. Windows currently terminates only the direct child;
wrapper descendants are not contained. Linux is live-tested; macOS and Windows are not. Cleanup
still runs if the plugin unloads before `registerMcpServer` resolves. Registrations
are ephemeral; server disable switches use stable plugin + local-server ids and
survive reloads and app restarts. Settings → MCP labels each server with its plugin
and retains the usual tool list, restart and disable controls.

Both tool discovery and dispatch enforce workspace scope. A scoped server is
hidden from chats without a matching workspace, including calls to a previously
known tool name. Plugins can inspect and call only their own registered servers
through the supported API, outside any chat. Plugin code remains unsandboxed;
this API is a typed lifecycle route, not a new security boundary. Remote/SSH
provider sessions receive no local registry tools. `scope: "plugin"` makes a
server available exclusively through its owner's `host.mcp` API: agents,
bridges and the gateway neither list nor dispatch it. An empty `workspaceIds`
array is rejected.

`toolDescriptions` is a per-raw-tool-name map of `{replace?: string, append?: string}`.
Replace runs first, then append adds a blank line and the supplied text. All
provider schemas, live lists and plugin UI lists see the same description.
Limits: 128 entries, 2,000 UTF-8 bytes per append/replace, and 16 KiB total for
keys plus text. Prefer short instructions for the specific tools that need them.

`toolPrefix` has 1–32 ASCII letters, digits, underscores or dashes. `mcp` and prefixes starting with `codemux` are reserved (case-insensitive). Registrations
may share it when their workspace scopes are disjoint. Models see
`<toolPrefix>__<rawToolName>` (Claude adds `mcp__codemux__`). Registry names
are bounded to 50 ASCII letters, digits, underscores or dashes, reserving 14
characters for that observed Claude envelope so full provider names fit 64.
If replacement or truncation is necessary, the name is truncated to 37 characters and suffixed with `_` plus 12 SHA-256 hex characters
of the complete original name. A collision visible in one workspace is refused.
HTTP URLs require HTTPS outside loopback, without embedded credentials or fragments. Loopback bypasses environment proxies; MCP HTTP clients do not follow redirects. Settings listings redact environment/header values, stdio arguments, URL queries and embedded URL credentials.
Commands/args are argv, never shell text. `cwd` must be absolute. Env/headers are
bounded to 128 entries and 64 KiB total (16 KiB per value).

For a live indicator, subscribe to activity:

```ts
ctx.host.mcp.onActivity("editor-project-a", (event) => {
  // {phase: "start" | "end", callId, toolName, prefixedName, workspaceId,
  //  isError?, durationMs?}. Exactly one end follows each start, including
  // errors, timeouts and disposal. Arguments and images are never copied.
});
```

The runtime follows all `tools/list` pages and re-lists on
`notifications/tools/list_changed`. Claude refreshes its live SDK server; the
HTTP gateway emits the same notification to connected clients. Claude catalog
swaps wait until the current turn ends, preserving in-flight tool results.
Claude also receives `codemux_list_plugin_tools` and `codemux_call_plugin_tool`:
use them when an Editor or another integration connects during the current turn,
while its newly added tools are absent from ToolSearch. Native refresh remains
available at the next safe boundary. Claude receives the two catalog tools only
when a plugin registration is visible to that workspace, even if its tool list
is still empty. Register the MCP server before its external app connects, even when the server
initially advertises zero tools. This puts the bridge in place for same-turn
flows. A first plugin loaded during a turn becomes available at the next safe
boundary. A hung refresh times out after five seconds; the next turn
resumes the conversation with the pending catalog. The list marks tools already in this
session's native snapshot with `directlyCallable`. Plugins loaded
after a chat starts use those same paths. OpenCode chats see only globally scoped plugin tools in this PR; workspace scoping for OpenCode follows in the gateway-identity PR.

**Codex:** its installed 0.162.1 app-server fixed dynamic schemas at thread start
in our probe. HTTP MCP notification/reload attempts did not update that running
thread's model tools. Codemux therefore offers initial plugin tools directly,
with their schemas, plus two stable tools in every local Codex chat:
`codemux_list_plugin_tools` and `codemux_call_plugin_tool`. They discover and call
the current workspace's tools, including ones that appear after an Editor
connects. The list includes `directlyCallable` for the initial snapshot. When the
visible plugin catalog changes, Codemux adds one bounded reminder to the next
provider turn, so models know to check the live catalog. It does not change the
user's message in the app transcript, queues, forks or provider handoffs. A new
chat is unnecessary for late tools. Older resumed Codex threads whose saved
snapshot predates these bridge tools need a new chat to acquire the bridge.

See [`examples/mcp-tools/`](./examples/mcp-tools/) for a worked plugin.

Claude bridge approvals use the target tool's native name and arguments. A deny
rule on that name also denies bridge calls; an allow for X does not allow Y.
Denied targets are omitted from the Claude list tool. The in-process call handler
requires a one-shot permission ticket for the exact call and arguments. If bare
mode, policy or workspace trust skips the hook, bridge calls fail closed with
a clear message to use the native tool or enable hooks. Bridge permission checks
accept at most 1 MiB of JSON input. Managed workflow
sessions exclude these two plugin tools.
The guard reads the CLI's effective permission rules through the installed SDK's
`getSettings()` control method. This runtime method is pinned by a compatibility
test; if it is unavailable or its response changes, calls fail closed with a
visible explanation to call the tool directly or update Codemux, including when
the permission object is absent. An empty session settings layer keeps that
object present on a clean CLI without replacing existing user/project rules.
The hook uses the CLI-reported current permission mode. Managed-only
permission state that the settings view cannot expose also fails closed.

Codex's registry tools, both native `dynamicTools` and bridge calls, use the
app-server's `item/tool/call` path. That path has no Claude-style per-tool
allow/deny rules or approval dialog in Codemux. Codex shell/file approval and
sandbox settings do not add per-tool approval to these app-hosted tools. The
bridge uses the same workspace visibility and dispatch checks as native tools.
MCP results can carry `structuredContent` alongside text and images. Codex's
dynamic-tool protocol has no structured-result field, so Codemux appends one
JSON text block when the server has not already included an equivalent JSON
text block. This generated block is limited to 64 KiB of UTF-8, including a
clear truncation marker when needed; original text and image blocks are kept.
Claude's SDK server and the gateway forward the full MCP result unchanged.

The catalog tools stay present in every local Codex chat because its dynamic
schemas cannot add them later.

Plugin stdio servers on Linux and macOS use a 2 MiB limit per JSON-RPC frame,
including image results. Windows and macOS shutdown behavior remains untested
live.

## Local processes, project files and plugin data

These typed desktop APIs are bound to the plugin incarnation. Plugins already
run with full authority; this is the supported, bounded route for CLI checks and
project detection. The web remote refuses these commands.

```ts
const executable = await ctx.host.process.which("unity"); // absolute path or null
const result = await ctx.host.process.run({
  command: "unity",
  args: ["status", "--project-path", projectPath, "--json"],
  cwd: projectPath,                  // absolute local path
  workspaceId,                      // optional locality check only
  env: { MY_OPTION: "value", OLD_OPTION: null },
  timeoutMs: 30_000,
  maxOutputBytes: 1024 * 1024,
});
// { code: number | null, stdout, stderr, timedOut, truncated, pipesHeldOpen, cancelled }
```

`run` passes argv directly, without a shell. `command` resolves on PATH (including
Windows PATHEXT), or names an executable path. Empty and relative PATH entries
are ignored; a repository's files cannot become PATH candidates through `cwd`.
On Windows an extensionless command uses PATHEXT, skipping extensionless shell
shims. Children inherit the app's
environment, with the app's usual AppImage environment cleanup; `env` overrides
values, and `null` removes a key. Windows batch files (`.cmd`/`.bat`) are refused
by `run`, including names with trailing dots or spaces, because launching them
would invoke a shell. Call the wrapped `.exe` (for example, `node.exe` with the
script) instead; `which` can still locate batch files. Do not pass untrusted
arguments to `cmd.exe /c`: its shell quoting differs from normal argv. Windows
`run` accepts `.exe` and `.com` programs, including `.exe` App Execution Aliases;
children use `CREATE_NO_WINDOW` to avoid a console flash.
Without `cwd`, the child uses the app’s current directory, not a workspace path.
Inputs allow up to 256 arguments (16 KiB each, 1 MiB combined) and 128 environment
overrides (16 KiB per value, 64 KiB combined).

The default timeout is 30 seconds (maximum 10 minutes), shared by executable
lookup, local cwd resolution, startup and execution. Filesystem lookup and OS
process creation run off the async workers. A lookup finishing after timeout or
disposal cannot launch a child; a process creation already in progress is
supervised until its result can be killed and reaped. The combined stdout and
stderr cap defaults to 1 MiB (maximum 8 MiB); excess bytes are drained and discarded,
with `truncated: true`. Timeout or plugin disposal kills the owned process group
on Linux and macOS, then waits for the child to exit. On normal launcher exit,
the host gives descendants 100 ms to detach, then kills any still in its group
before reaping the launcher. Windows currently kills the
direct child only. A child that deliberately starts a separate session, such as
an Editor launched by a CLI, remains independent. Inherited output pipes get at
most 100 ms to collect after the launcher exits; `pipesHeldOpen` signals that a
pipe stayed open. The host keeps reading and discarding until EOF so an Editor's
later log writes do not hit a closed pipe. `truncated` reports the output cap or
a pipe read failure, independently of `pipesHeldOpen`. The cap counts raw bytes;
invalid UTF-8 is replaced without dropping valid text. Linux lifecycle tests
cover a normal grandchild, delayed detachment and detached stdout writes.
Windows and macOS have not been tested live.

`workspaceId` is optional on `run` and every filesystem call. If supplied, it
must name an open, local workspace. If omitted, the operation runs locally even
when the selected workspace is remote. The id validates locality only: it does
not select, change or check a path against the workspace directory.
**Filesystem paths and `cwd` must be absolute and refer to this machine.** These APIs do not read files or launch programs over SSH.

```ts
await ctx.host.fs.exists(projectPath, { workspaceId });
const entries = await ctx.host.fs.listDir(projectPath, { workspaceId });
// [{ name, kind: "file" | "directory" | "symlink" | "other" }]
const text = await ctx.host.fs.readText(absolutePath, { maxBytes: 1024 * 1024, workspaceId });
```

`readText` reads UTF-8 regular files and refuses files larger than `maxBytes`
(default 1 MiB, maximum 8 MiB). `listDir` refuses more than 10,000 entries.
These filesystem calls (and `which`) run on blocking workers. They have no
per-operation timeout: an OS call stalled on a dead mount cannot be interrupted.
App-exit cleanup waits at most three seconds for such calls.

For CLI policies and manifests, the only supported write route is plugin-owned data:

```ts
const dir = await ctx.host.fs.dataDir();
const policyPath = await ctx.host.fs.writeDataFile("policy.json", JSON.stringify(policy));
await ctx.host.fs.removeDataFile("policy.json");
```

Files live under `<app data>/plugin-data/<pluginId>/`, separate from installed
code. `name` is one segment: ASCII letters, digits, dots, underscores or dashes,
at most 128 bytes, no leading dot, no `..`, and no Windows device names. Links
are refused. Writes are atomic, capped at 1 MiB, and use owner-only directory
and file permissions on Unix. Data is retained on disable, reload, update **and
uninstall**, matching existing `ctx.storage` retention. Reinstalling the same
plugin id can reuse it. `removeDataFile` deletes only that plugin's named file.
On Windows, another program's open handle can prevent replacement. Sharing or
access failures retry with short backoff for up to one second, then return an
error asking you to close the file or check permissions and try again. A failed
write removes its temporary file and leaves the previous data file intact.
Acquiring the data directory sweeps abandoned writer temp files older than one
day, without following links; recent writes and named data files are retained.
Returned Windows paths use ordinary absolute spelling when safely equivalent;
paths that require Windows verbatim syntax retain it. Containment checks always
use canonical paths internally.

`ctx.host.workspace.list()` returns the open workspaces as `{ id, name, path,
remote }[]`. `ctx.host.workspace.onListChange(fn)` notifies when that list
changes and unsubscribes on disposal. Use it to prune workspace registrations;
`current()` and `subscribe()` still describe the selected workspace.

For install locations that a running app's PATH does not yet include, use
`await ctx.host.os.info()`:

```ts
// Exact Rust OS/architecture spellings, not Node's "darwin"/"arm64".
{
  platform: "linux" | "windows" | "macos",
  arch: "x86_64" | "aarch64",
  homeDir: string | null,
  configDir: string | null,
  dataDir: string | null,
  localDataDir: string | null,
}
```

These are `std::env::consts::OS` / `ARCH` on supported desktop targets;
`info()` refuses other targets rather than returning an undocumented value.
`configDir` uses `$XDG_CONFIG_HOME` (default `~/.config`) on Linux,
`~/Library/Application Support` on macOS, and roaming AppData on Windows.
Directories use native `dirs` resolution: Windows profile, roaming AppData and
local AppData; Linux XDG data home (default `~/.local/share`); macOS
`~/Library/Application Support` for both data directories. No general environment
reader is exposed. `dataDir` here is the OS directory, whereas `fs.dataDir()` is
the plugin's private directory beneath the app's data directory.

On Linux the GUI inherits its launcher's PATH; it does not import a login shell's
PATH. A desktop session may omit `~/.local/bin`, so an integration can fall back
to `homeDir/.local/bin` when `which` misses a newly installed CLI. On Windows,
`which` accepts non-directory reparse points, including zero-byte App Execution
Aliases, and `run` passes that path through to CreateProcess. Classification and
PATHEXT lookup have unit coverage; Windows execution remains untested live.
