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
child. `shutdown: "immediate"` skips that grace: on Linux it immediately kills
the owned process group; on other platforms it terminates the owned child.
Linux is live-tested; macOS and Windows are not. Cleanup
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

`toolPrefix` has 1–32 ASCII letters, digits, underscores or dashes. Registrations
may share it when their workspace scopes are disjoint. Models see
`<toolPrefix>__<rawToolName>` (Claude adds `mcp__codemux__`). Registry names
are bounded to 50 ASCII letters, digits, underscores or dashes, reserving 14
characters for that observed Claude envelope so full provider names fit 64.
If replacement or truncation is necessary, the name is truncated to 37 characters and suffixed with `_` plus 12 SHA-256 hex characters
of the complete original name. A collision visible in one workspace is refused.
HTTP URLs must explicitly use http(s), without embedded credentials or fragments.
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
swaps wait until the current turn ends, preserving in-flight tool results. Plugins loaded
after a chat starts use those same paths.

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
