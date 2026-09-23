# @codemux/plugin-sdk

API 1.0.0 for optional CodeMux feature plugins. Authors use TypeScript and Preact
JSX. Code runs in the bundled QuickJS child, never the desktop renderer. This
package is an implementation preview, not a declaration that release gates passed.

Build this package with `npm ci --ignore-scripts && npm run build`, then
`npm pack --ignore-scripts`. Install the resulting tarball into your author project
alongside a packed `@codemux/plugin-cli`. No app checkout is required there.
`codemux-plugin init` creates a starter project; this README is the API reference.

```tsx
import {definePlugin, Stack, Text, Button, PluginError} from '@codemux/plugin-sdk';
export default definePlugin({activate(ctx) {
  ctx.commands.register('open', context => ctx.panels.open('brief', context));
  ctx.panels.register('brief', () => <Stack spacing="md">
    <Text>A separately distributed plugin</Text>
    <Button onPress={async event => {
      try {
        await ctx.composer.appendText(event.context, 'A brief');
      } catch (error) {
        // NO_COMPOSER, INTERACTION_REQUIRED, CONTEXT_STALE, ...
        if (!(error instanceof PluginError)) throw error;
      }
    }}>Add to draft</Button>
  </Stack>);
}});
```

The project needs `manifest.json`, `README.md`, `LICENSE` and the entry file
`src/index.tsx`, whose default export is the `definePlugin` result. Declare each
ID in `manifest.json` before registering it, and declare `composer.append` for
this example. The bundle contains all dependencies; there are no runtime imports,
`process`, `require`, `fetch` or browser DOM. Browser libraries that need a real
DOM are unsupported. Hooks (`useState`, `useEffect`, `useMemo`, `useCallback`,
`useRef`, `useReducer`) are Preact hooks. `schema/manifest.json` is generated from
the desktop's Rust contract; the desktop also validates the rules below.

## Manifest

| Field | Rules |
| --- | --- |
| `format`, `manifestVersion`, `entry` | `"codemux.feature-plugin"`, `1`, `"plugin.js"` |
| `id` | `publisher.name`; each part 2–40 of `a-z 0-9 -`, starting with a letter; not a Windows device name (`con`, `prn`, `aux`, `nul`, `com1`–`com9`, `lpt1`–`lpt9`). Fixed once users install it |
| `name`, `description`, `author.name`, `license` | 1–80, 1–240, 1–80 and 1–100 characters, no control characters |
| `author.url`, `repository` | HTTPS URLs without credentials |
| `version` | SemVer such as `1.0.0` or `1.1.0-beta.1`; no `v` prefix or spaces |
| `api` | Comma-separated comparators that include `1.0.0`, such as `^1.0.0` or `>=1.0.0, <2.0.0`. A bare version means `^`. `\|\|`, hyphen ranges and space-separated comparators are rejected |
| `platforms` | One or both of `linux-x64`, `windows-x64` |
| `permissions` | `workspace.read`, `git.read`, `composer.append`, `external.open` |
| `http` | Up to 20 `{origin, methods, credential}`. `origin` is an exact lowercase `https://host` on port 443: no path, port, wildcard, IP address or trailing dot. `methods` from `GET`, `POST`, `PUT`, `PATCH`, `DELETE`. `credential` is `null` or a credential ID for the same origin |
| `credentials` | Up to 20 `{id, label, origin, type: "bearer"}`, one per origin, each referenced by its origin's `http` entry. The user enters the token in Settings; plugins never read it |
| `contributes` | `commands` (≤20, `{id, title, requiresWorkspace}`), `panels` (≤8), `composerActions` (≤8) and `composerViews` (≤4), each `{id, title, icon}`. IDs are 1–40 of `a-z 0-9 -` starting with a letter; titles 1–80 characters |
| `settings` | Up to 50 `{id, type, label, default}` with `type` `boolean`, `string` (≤4 KiB, default `""`), `integer` (also `min` and `max`) or `enum` (also 1–50 `values` containing the default) |

The whole manifest is at most 64 KiB. Icons are `file-text`, `git-branch`,
`github`, `list`, `check`, `info`, `settings`, `book-open`, `link`, `refresh-cw`,
`plus`, `circle-alert`, `folder`, `terminal`, `code` and `search`, also exported as
`ICONS` and the `IconName` type. A package holds `manifest.json`, `plugin.js`
(≤5 MiB), `README.md`, `LICENSE` and optional `NOTICE` and `source.map`.

## Contributions and context

`activate(ctx)` registers a callback for every declared contribution before it
returns (or before its promise resolves). An undeclared or duplicate ID throws,
and a declared ID left unregistered fails activation.

| Registration | Callback |
| --- | --- |
| `ctx.commands.register(id, handler)` | `(context) => void \| Promise<void>`; shown in the command palette |
| `ctx.composerActions.register(id, handler)` | Same shape; shown in the composer's add-on menu |
| `ctx.panels.register(id, render)` | `({viewId, context}) => children`; a right-panel view |
| `ctx.composerViews.register(id, render)` | Same shape; an accessory above the composer |

Each registration returns a disposer. Contributions are fixed for a running
plugin generation, so disposing after activation only stops the callback: the
entry stays listed, invoking a disposed command does nothing, and a disposed view
renders empty. Registering the same ID again restores it. Workspace and settings
subscriptions also return disposers. The app owns final cleanup regardless of
author cleanup failures.

A `ContextHandle` is an opaque string checked by the desktop. Views receive one
bound to their workspace and composer. Commands run from the palette, and UI
events such as `onPress`, carry a fresh handle that also holds a single-use user
interaction, valid for 10 seconds and not extended by awaiting other work. Pass
the handle from the event that caused an action; never store handles. When the
project changes, mounted views are unmounted and `workspace.subscribe` callbacks
receive the new handle (or `null`). An idle plugin stops after 60 seconds without
a mounted view, so keep state that must survive in storage.

## Host API

Every method returns a promise. Failures reject with `PluginError`, whose `code`
is one of `PERMISSION_DENIED`, `CONTEXT_STALE`, `NO_WORKSPACE`,
`REMOTE_UNSUPPORTED`, `NO_COMPOSER`, `INTERACTION_REQUIRED`, `NOT_A_GIT_REPO`,
`INCOMPATIBLE_API`, `RESOURCE_LIMIT`, `TIMEOUT`, `PLUGIN_STOPPED`,
`INVALID_MESSAGE`, `CREDENTIAL_REQUIRED`, `NETWORK_DENIED` or
`STORAGE_UNAVAILABLE`, with a readable `message`.

| Method | Result | Requires | Typical errors |
| --- | --- | --- | --- |
| `workspace.current(context)` | `{id, name, rootName, location: 'local'}` or `null`; never a path | `workspace.read` | `CONTEXT_STALE`, `REMOTE_UNSUPPORTED` |
| `workspace.subscribe(callback)` | Disposer; `callback(context \| null)` on project change | `workspace.read` | — |
| `git.summary(context)` | `{branch \| null, ahead, behind, staged, unstaged, untracked, conflicts, paths, truncated}`; up to 500 relative paths | `git.read` | `NOT_A_GIT_REPO`, `NO_WORKSPACE`, `TIMEOUT` |
| `panels.open(id, context)` | Opens one of this plugin's panels | Live interaction | `INTERACTION_REQUIRED`, `CONTEXT_STALE`, `NO_WORKSPACE` |
| `composerViews.open(id, context)` | Opens this plugin's accessory on the bound composer | Live interaction | `NO_COMPOSER`, `INTERACTION_REQUIRED` |
| `composer.appendText(context, text)` | Appends up to 32 KiB to the bound draft, keeping its content; resolves to the draft revision | `composer.append`, live interaction | `NO_COMPOSER`, `INTERACTION_REQUIRED`, `CONTEXT_STALE` |
| `settings.get()` | Current values of the declared settings, defaults included | — | — |
| `settings.subscribe(callback)` | Disposer; `callback(settings)` after the user changes them | — | — |
| `storage.get(scope, key)` / `set(scope, key, value)` / `delete(scope, key)` | JSON or `null`; `set` and `delete` resolve after the write commits | — | `STORAGE_UNAVAILABLE`, `RESOURCE_LIMIT`, `NO_WORKSPACE` |
| `http.fetch(context, {origin, path, method, headers?, body?})` | `{status, headers, body}` | A matching `http` entry | `NETWORK_DENIED`, `CREDENTIAL_REQUIRED`, `RESOURCE_LIMIT`, `TIMEOUT` |
| `links.open(url, context)` | Opens an HTTPS URL in the browser | `external.open`, live interaction | `INTERACTION_REQUIRED`, `NETWORK_DENIED` |
| `ui.notify(message)` | Shows a short attributed notification (≤500 characters) | — | `RESOURCE_LIMIT` after three per minute |

Storage `scope` is `{scope: 'global'}` or `{scope: 'workspace', context}`. Keys
are at most 128 ASCII characters, values at most 64 KiB of JSON, and each plugin
at most 5 MiB. `http.fetch` takes an absolute path starting with one `/` (query
included), a request body up to 256 KiB, and safe request headers; the desktop
attaches the declared credential when the user configured one. It never follows
redirects, returns only `content-type`, `etag`, `retry-after` and `x-ratelimit-*`
response headers, and limits bodies to 512 KiB, four concurrent requests and
10 MiB per minute. During `activate` a plugin may register, read settings and use
storage, but network, notifications, panels and composer calls are refused.

## Faults and limits

A stable host rejection is an ordinary outcome. When a command handler, composer
action, UI callback or subscription callback returns a promise that rejects with
a `PluginError`, the SDK records a short diagnostic and the plugin keeps running.
Still, catch errors where users need to see them, as the example above does. A
click can also cross an update: an event for a callback that a newer render
removed, or for a view that has closed, is ignored with a short diagnostic.

Everything else is a runtime fault. A synchronous throw from any callback or
render, a promise that rejects with anything other than a `PluginError`, an
unhandled rejection elsewhere (for example an uncaught promise in `useEffect` or
a timer), invalid UI and exceeded resources stop the plugin. It is disabled until
the user chooses Retry in Settings → Add-ons; it is never restarted
automatically.

| Resource | Limit |
| --- | --- |
| Host requests | 16 outstanding; 15 s timeout (30 s for HTTP); 20 per second with a burst of 20, and 100 per minute |
| UI updates | 30 batches per second, 1,000 mutations per batch, four mounted views |
| Views | 2,000 nodes, depth 32, 256 KiB of serialized state each |
| Console | Five entries per second, up to 1,024 characters each; the app records only their size |
| Execution | 250 ms per callback (1 s during activation), 1 s of JS per rolling 5 s, 64 MiB heap |
| Timers | 128 live timers; intervals of at least 100 ms |

The SDK paces its own traffic below these quotas. Requests beyond the
per-second rate wait briefly in order; once the per-minute budget is spent they
reject locally with `RESOURCE_LIMIT` instead of reaching the host. UI updates
are coalesced into at most about 16 batches per second, keeping only the latest
value of each property, and console entries above the rate are dropped. Rapid
input therefore never stops a plugin, but avoid a host request per keystroke;
save on blur, on a button, or after a pause.

## Components

Components accept token properties, never CSS or HTML. Each has its own props
type (`StackProps`, `TextFieldProps`, ...); values outside these ranges fail type
checking and are rejected by the desktop, which stops the plugin.

| Component | Props |
| --- | --- |
| `Stack` | `direction` (`vertical` \| `horizontal`), `spacing` (`none`, `xs`, `sm`, `md`, `lg`), `align` (`start`, `center`, `end`, `stretch`) |
| `Grid` | `columns` (1–12), `spacing`, `align` |
| `Card` | `title`, `spacing`, `align` |
| `Text`, `Badge` | Text children |
| `Heading` | `level` (1–6), text children |
| `Markdown` | Markdown text children; HTML and images are dropped, and HTTPS links open through the host (`external.open`) |
| `Button` | `label` or children, `disabled`, `onPress` |
| `TextField`, `TextArea` | `label`, `value`, `placeholder`, `disabled`, `onChange` (`event.value` is a string) |
| `Select` | `label`, `options` (`{label, value}[]`), `value`, `disabled`, `onChange` |
| `Checkbox`, `Switch` | `label`, `checked`, `disabled`, `onChange` (`event.value` is a boolean) |
| `Tabs` | `label`, `options`, `value`, `disabled`, `onChange`; children are the selected panel |
| `List` | `items` (up to 500 strings), `label` (the accessible name) |
| `Table` | `headers`, `rows` (up to 500 rows of up to 20 strings), `label` |
| `Progress` | `value`, `max`, `label` |
| `Icon` | `name` (an `IconName`), `label`, `color` |
| `Divider` | — |
| `EmptyState` | `title`, children |

All components except `Icon` and `Divider` also take `size` (`xs`–`lg`), `color`
(`default`, `muted`, `success`, `warning`, `danger`, `accent`), `width` and
`height` (`auto`, `full`). Strings are limited to 4 KiB (32 KiB for text and
field values). Every callback receives `{context, value}`.

`TextField` and `TextArea` keep the text locally while the user types and report
every change through `onChange`. A `value` you render replaces that text only
when it differs from the last value the field reported, so echoing the value
back never moves the caret or drops keystrokes, and a field without `value` is
uncontrolled. `Select`, `Checkbox`, `Switch` and `Tabs` show the `value` or
`checked` you render, so update your state in `onChange`.

A batch that breaks a rendering limit is rejected whole and stops the plugin;
intermediate states must fit as well. Native validation has a 50 ms budget per
batch. Prefer small updates to existing content over replacing large subtrees.
These ceilings are bounds, not a guaranteed update rate on every machine.
