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
| `LANDING_BACKDROP_AREA` | Behind the new-thread landing, `pointer-events: none` | `render({ onPulse, calmRect })` |
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
| `host.reducedMotion()` | The user's reduce-motion setting. Respect it. |
| `host.notify({ kind?, message, description? })` | A toast. It is not stamped with your plugin's name, so say who you are in the message. (Load errors, render crashes and failed commands are reported by the app, named after your plugin.) |
| `host.openPage(id)`, `host.openSettings(section?)`, `host.openRightPane(id)` | Navigation. Ids are full contribution ids, `"<pluginId>:<localId>"`. |
| `host.workspace.current()`, `host.workspace.subscribe(fn)` | The active workspace (`workspace_id`, name, path, …). |
| `host.settings.get(key)`, `host.settings.subscribe(key, fn)` | App settings, read-only. |
| `host.composer.insertText(text)`, `.getDraft()`, `.focus()` | The focused composer (or the only one). Return `false`/`null` when none is mounted. |
| `host.chat.onEvent(fn)` | Every thread's provider events, including background threads with no pane open. `usage_recorded` carries `cacheWriteTtl` (`"1h"` / `"5m"`, Claude only) and `subagent`. The native tap runs only while someone listens. |
| `host.chat.getThread(id)`, `host.chat.listThreads()`, `host.chat.sendTurn(id, text)` | Read a thread; send a turn the same way the send button does. |
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
- **Not on the web remote.** Plugins run only in the desktop window; the web client never loads them, and `plugins_*` commands are refused there.

## Bundled plugins

The app itself ships plugins in `src/plugins/<id>/`:

- `meta.ts` is loaded eagerly: id, name, description and `defaultEnabled`.
- `plugin.ts(x)` is a lazy chunk that is fetched only when the plugin is on.
- Bundled code may import only `@codemux/sdk` and `react`. This is enforced by `src/plugins/import-fence.test.ts`.

`src/plugins/example` uses every area and is off by default.

## Decisions

Choices the spec left open are recorded one per line in
[`DECISIONS.md`](./DECISIONS.md), each with how to revert it.
