# CodeMux feature plugins — implementation specification

Revision 2 · 18 September 2026 · Ready to implement against the inspected baseline.

This document is the engineering handoff. It supersedes the earlier theme/footer and single-panel prototype. It specifies work to build; it does not claim that work has shipped. The product overview and interactive diagram on the PassPage explain this specification, but this document governs implementation details.

## 1. Outcome and release boundary

Build optional, separately distributed feature plugins for the official CodeMux desktop app. People keep receiving official CodeMux updates. Authors use a documented SDK instead of modifying app source. Users manage packages in Settings → Add-ons and discover community packages on the CodeMux website.

The first release must support all of these together:

- Commands in the existing command palette, with plugin attribution.
- Custom interactive views in the existing right-panel deck, authored with JSX and SDK components.
- Actions and an expandable accessory area in the existing chat composer.
- Scoped access to local project metadata and read-only Git summaries.
- HTTPS service integrations through a permission-checking host API, with optional bearer credentials managed by CodeMux.
- Private local plugin settings/state, installation, enable/disable, manual updates, removal, diagnostics, and recovery.
- A public SDK, author starter, two separately built example packages, and a curated website catalog/submission workflow.

The app, manager, SDK, examples, catalog, and author documentation form one delivery. A checklist panel or working installer alone is not completion.

Keep existing Appearance/Theme Studio and footer customization separate. Do not add theme or footer-preset package types. Do not change core UI when no plugin contributes to it. No automatic account synchronization, plugin billing, OAuth platform, third-party native binaries, arbitrary shell commands, arbitrary file writes, replacement of internal React components, global stylesheet injection, or automatic chat submission in this release. Skills and MCP remain their existing products.

Target the current release matrix: Linux x86_64 and Windows x86_64. macOS, ARM targets, browser remote clients, and remote-workspace plugin execution are later work. Preserve portable abstractions, but do not advertise untested platforms.

## 2. Research findings and decisions

CodeMux was inspected at `797a834c7a9aea658de5f97b7f97f1fa3d32c287` (0.22.8). Hermes was inspected at local commit `498abb677e`. The website checkout, `codemux-sitev2`, was inspected at `447f89b`. Refresh file locations if the branches move; preserve the contracts below.

| Finding | Implementation consequence |
| --- | --- |
| Hermes has a contribution registry, SDK, disposal tracking, plugin inventory, and a catalog of separately maintained packages. | Adopt the supported-API and contribution-lifecycle model. |
| Hermes’s desktop runtime loader evaluates plugins in its renderer and explicitly describes that loader as having full app authority. | Do not copy its execution boundary. React error boundaries cannot stop an infinite loop. |
| CodeMux already has a right-panel registry and persisted per-workspace deck. | Extend that deck rather than inventing a popup or a new workspace shell. |
| CodeMux’s composer is controlled through props, and live and draft chat surfaces have different draft ownership. | Add one mounted-composer adapter; never write only to a global thread store. |
| The command palette currently reads a fixed command array. | Merge namespaced plugin descriptors into that list, preserving all core commands. |
| The web-remote dispatcher can route the desktop command surface. | Deny add-on management and bridge commands at the remote dispatcher, not just in the UI. |
| The website is a Next.js application with documentation content and no plugin catalog yet. | Add static catalog routes and submission instructions; no new account database is required. |
| Existing native release CI builds Linux and Windows. | Package and test the plugin host on both before enabling general availability. |

### Architecture decision

Use **one supervised QuickJS process per active plugin**, backed by a standalone Rust host. Use **Remote DOM with a validated component allowlist** to render plugin UI in CodeMux. Authors write TypeScript and Preact JSX; the app continues using its existing React renderer. Plugin functions never execute in the main renderer.

The first author toolchain pins `rquickjs = 0.13.0`, `@remote-dom/core = 1.11.1`, `@remote-dom/preact = 1.3.0`, `preact = 10.29.8`, and `esbuild = 0.28.2`, which were used by the probe. Put exact versions in lockfiles. Reassess a pin only for a demonstrated compatibility/security issue and record the change. Do not enable rquickjs custom allocator features without reimplementing and testing its memory limit.

A standalone Linux probe successfully rendered remote elements with Preact, serialized a button callback, emitted a simulated composer request, and produced a state update. An endless loop was interrupted in approximately 252 ms; a 128 MiB allocation was rejected under a 64 MiB heap limit. `process`, `require`, `fetch`, and Tauri globals were absent. [Probe results](research-probe/probe-results.json) and [reproduction instructions](research-probe/README.md) are included.

This proves basic composition of the selected libraries. It does not prove CodeMux integration, host-side validation, Windows behavior, safe installation, or adversarial security. Those are explicit implementation tests below, not reasons to reopen the entire product architecture.

## 3. Runtime, rendering, and resource boundaries

### Native host and process supervision

Create `src-tauri/addon-host/` as a small independent Cargo crate. It must not link the Tauri application crate. The main Rust app supervises it through stdin/stdout, never a public socket. Build the host from trusted app source and ship it with CodeMux; plugin archives contain no executable native files.

Use QuickJS standard language intrinsics without OS, filesystem, module-loader, network, or process bindings. Provide only the SDK bridge, bounded console logging, timers, UTF-8 helpers, and the minimal Remote DOM polyfill. Accept one author bundle as JavaScript source. Never load cached QuickJS bytecode from packages, run package scripts, resolve runtime imports, or use Node installed on the user's machine.

The app resolves a known bundled host path. Clear inherited environment variables; retain only required platform startup variables. Set an empty private working directory and close unrelated inherited handles. The host exits on parent-pipe EOF. App shutdown kills and reaps only its recorded children. A process boundary and a constrained JS environment are fault-containment measures; they are not a claim of protection against every native-engine vulnerability.

Start hosts lazily when a declared command, visible panel, or composer accessory needs execution. Static manifest descriptors make commands discoverable before activation. Stop an idle host after 60 seconds with no mounted UI or pending call. Keep stored enablement separate from runtime activity. At most eight hosts run simultaneously; evict idle hosts first, otherwise report `RESOURCE_LIMIT` without blocking core UI or silently disabling unrelated plugins.

### Wire protocol

Use UTF-8 newline-delimited JSON-RPC 2.0 with integer request IDs, bounded framing, and protocol version `1`. Authenticate identity by the child process handle. Never accept a plugin-supplied identity as authority. Each activation has a random generation ID; every message carries that generation. Reject messages from disposed generations before dispatch. Bound all incoming queues; allow at most 20 host requests/sec with a burst of 20 and 100/minute per plugin. Truncate logs to 4 KiB/entry and five entries/sec. Repeated malformed messages or quota violations stop the offending generation; do not accumulate unbounded rejection responses.

Parent methods: `initialize`, `activate`, `command.execute`, `view.mount`, `view.unmount`, `ui.event`, `workspace.changed`, `settings.changed`, `deactivate`. Child methods: `ready`, `ui.patch`, `host.request`, `log`. Only `host.request` reaches the broker; it takes a documented SDK operation and JSON parameters. Responses use `result` or JSON-RPC `error`, with a stable SDK error name in `error.data.code`. Unknown methods are rejected.

The trusted SDK adapter serializes Remote DOM callbacks into IDs; functions never cross the pipe. On receipt, the app validates a complete mutation batch, then builds a bounded normalized tree. Host event handlers return only typed event payloads and callback IDs to the child. Do not pass child records directly into a DOM renderer or revive arbitrary objects/functions. Callback IDs belong to one generation, view, and current node and are released on replacement/unmount.

Use a separate add-on transport wrapper. The existing `JsonRpcChild` helper has useful shutdown concepts but inherits environment variables and was designed for trusted provider processes. Do not reuse it unchanged for untrusted input or change its existing providers’ behavior as a side effect.

### Initial limits

| Resource | Required initial bound |
| --- | --- |
| JS heap / stack | 64 MiB heap; 512 KiB stack per runtime |
| Active JS execution | 250 ms per synchronous callback or microtask-drain burst; 1 s during initial activation |
| Supervisor response deadline | 2 s for activation/progress acknowledgement; terminate a nonresponsive child |
| Async host request | 15 s default; HTTP 30 s; at most 16 outstanding per plugin |
| Framing | 1 MiB per line, enforced while reading, not after allocating the whole line |
| UI tree | 2,000 nodes/view; depth 32; 256 KiB serialized live tree/view; four mounted views/plugin |
| UI traffic | At most 1,000 mutations/batch and 30 applied batches/sec; bounded queue of two batches |
| Callbacks / timers | 4,096 live callbacks and 128 timers/plugin; timer interval at least 100 ms |
| CPU / process growth | 1 s executing JS per rolling 5 s; stop on repeated excess. Parent monitors child RSS every 500 ms and stops it above 192 MiB. |
| Logging | 64 KiB in-memory ring per plugin; sanitize and truncate entries; no automatic telemetry |

Enforce CPU deadlines across promise jobs, not a fresh budget for every job. The trusted runtime acknowledges an event when it is accepted and reports when its synchronous work yields; an async handler may then await a host call without holding an acknowledgement open. Waiting for HTTP does not spend active-JS budget. Mutation overflow faults the plugin; never drop arbitrary patches and continue with a corrupted tree. Virtualize table/list rows in the trusted renderer. Resource defaults may be tuned using recorded benchmarks, but limits must remain finite and tested.

### Supported UI

Expose these SDK components: `Stack`, `Grid`, `Card`, `Text`, `Heading`, `Markdown`, `Button`, `TextField`, `TextArea`, `Select`, `Checkbox`, `Switch`, `Tabs`, `List`, `Table`, `Badge`, `Progress`, `Icon`, `Divider`, `EmptyState`. They map to existing CodeMux components or small trusted adapters.

Allow bounded layout props: spacing tokens, columns 1–12, alignment, width/height within the assigned view, typography tokens, semantic colors, and predefined icon names. Themes flow into these components through the app's own tokens. No raw HTML, script, arbitrary tags, style strings, URL-valued CSS, external image loads, global selectors, or arbitrary SVG. Markdown disables embedded HTML/images and routes links through the broker. No plugin-defined portal outside its assigned surface.

This is a composable UI SDK with real host operations. It is not a frozen one-panel text/button schema. Custom charts or browser-specific libraries may need future SDK components. Do not claim full browser DOM or drop-in compatibility with every React library.

## 4. Public SDK and first integration surfaces

Create `packages/plugin-sdk` and `packages/plugin-cli`. Package names are `@codemux/plugin-sdk` and `@codemux/plugin-cli`; use packed local packages during development, then the project's normal authorized publication workflow. SDK version is independent of the CodeMux app version. First supported API range is `^1.0.0`.

The SDK exports `definePlugin`, the UI components, types, and supported hooks. An author exports `definePlugin({ activate(ctx), deactivate?() })`. The build tool emits an IIFE that registers this object through a host-provided registration function. All dependencies and SDK adapters are bundled; no runtime import resolution.

`activate` registers callbacks for IDs declared in the manifest. Registration methods return disposers. Unknown or duplicate IDs fail activation; IDs are namespaced as `publisher.name/local-id`. Core IDs cannot be overridden. Registration cleanup is host-owned even if `deactivate` throws or never returns.

### Contribution contract

| Contribution | Public registration | Placement and rules |
| --- | --- | --- |
| Command | `ctx.commands.register(id, handler)` | Command palette group “Add-ons”; attributed label; no default shortcut hijacking |
| Panel | `ctx.panels.register(id, render)` | Existing right-panel deck; user opens via `+` or a command; never auto-open at startup |
| Composer action | `ctx.composerActions.register(id, handler)` | Existing attachment/actions menu under “Add-ons”; may open its accessory or perform its granted action |
| Composer accessory | `ctx.composerViews.register(id, render)` | One expandable area above the composer footer, opened by a user action; no replacement of Send/Stop/model controls |
| Settings | `ctx.settings` plus manifest declarations | Host-rendered fields under the plugin’s Settings → Add-ons detail |

Persistent panel ID format is `addon:<publisher.name>:<panel-id>`. Define explicit `isAddonPane` and `isCorePane` predicates; the existing “not a doc pane implies core pane” rule must change. Save panel preference IDs, not plugin state/JS. At restore, filter missing/disabled/incompatible IDs and choose a valid core fallback. A failed panel shows a bounded host-owned diagnostic until closed; disabling/removing strips its pane IDs.

Command/action handlers have the shape `(context: ContextHandle) => void | Promise<void>`. View renderers receive `{viewId, context: ContextHandle}` and return Preact component children. SDK UI event callbacks receive a context handle carrying the host-issued interaction; authors do not generate interaction tokens. `ContextHandle` is an opaque string whose authority is checked in Rust, not inferred from its TypeScript type.

Every mounted view and executed command receives a host-generated context handle. It binds generation, workspace, optional composer instance, and current context revision. The host, not the plugin, supplies paths and thread identities. On project switch, invalidate old handles, cancel pending workspace calls, unmount old workspace views, and create new handles. Clear a view before new project data arrives so stale content is not shown as current.

### Host API

| SDK operation | Result / behavior | Permission |
| --- | --- | --- |
| `workspace.current(context)` | `{id, name, rootName, location: 'local'}` or null; no absolute path | `workspace.read` |
| `workspace.subscribe(callback)` | Bound context-change notifications only; disposer; no full app store | `workspace.read` |
| `git.summary(context)` | Branch/null, ahead, behind, staged/unstaged/untracked/conflict counts, up to 500 changed relative paths, `truncated` flag | `git.read` |
| `panels.open(id, context)` | Opens only this plugin’s declared panel in the bound workspace | Own contribution |
| `composerViews.open(id, context)` | Opens only its declared accessory for a live mounted composer | Own contribution + live interaction |
| `composer.appendText(context, text)` | Append literal text to the bound current draft, preserving existing input; return resulting revision | `composer.append` + live interaction |
| `settings.get()` / `settings.subscribe(cb)` | Validated host settings, no secrets; subscription disposer | Private namespace |
| `storage.get/set/delete(key, value?)` | JSON only, private global namespace or explicit current-workspace namespace | Private namespace |
| `http.fetch(context, request)` | Bounded status, selected headers, UTF-8 response body; no cookies | Manifest HTTP grants |
| `links.open(url, context)` | Host opens an HTTPS URL after a real interaction; plugin attribution | `external.open` + live interaction |
| `ui.notify(message)` | Small attributed notification, rate limited to three/minute | Private namespace |

Storage calls take `{scope: "global"}` or `{scope: "workspace", context}` followed by the key and optional value. `get` returns JSON or null; `set/delete` resolve only after the broker transaction commits. Workspace subscriptions receive the new context handle or null and return a disposer. Settings subscriptions receive the validated current settings object and return a disposer. Host promises reject with the stable errors below.

No generic `invoke`, command execution, arbitrary path parameter, access to other plugins, token retrieval, transcript stream, draft reading, prompt sending, or unrestricted state store is exposed.

For Git, resolve the workspace root from authoritative Rust state and require a local workspace. Use a dedicated bounded read-only runner (`git --no-optional-locks`, porcelain status with NUL separators, no pager/fsmonitor/hooks/optional external diff). Share parsers where suitable, but do not route to existing arbitrary-path command handlers. Use a 5 s process timeout, bounded output, kill/reap on cancellation, and cap concurrent Git jobs at two/plugin. Cache summaries for 1 s. A symlinked root is canonicalized once against the authorized workspace; plugins cannot provide paths. Non-Git workspaces return `NOT_A_GIT_REPO`, detached HEAD has `branch: null`.

### Composer correctness

Implement `useAddonComposerAdapter` at the controlled `Composer` boundary, shared by `AgentChatPane` and `DraftChatSurface`. Bind to that instance’s `draft` and `onDraftChange`, including pre-thread drafts. Appending must use the latest draft, never a stale captured string, and preserve unsent user content and attachments. Append at the end with a newline separator when needed; maximum insertion 32 KiB. It must not send, materialize a session, change the provider, or redirect text to another thread.

On a trusted button/menu event the broker issues a single-use interaction token, valid for 10 s, bound to plugin generation and target context. The SDK transports this token implicitly. Network access or asynchronous work does not extend it. A stale target, changed workspace, expired token, closed composer, or disabled chat GUI rejects the action; nothing is inserted elsewhere. Actions that cannot run show a specific unavailable reason. Composer features are absent when chat GUI is off; panel/command features can still work.

Stable errors: `PERMISSION_DENIED`, `CONTEXT_STALE`, `NO_WORKSPACE`, `REMOTE_UNSUPPORTED`, `NO_COMPOSER`, `INTERACTION_REQUIRED`, `NOT_A_GIT_REPO`, `INCOMPATIBLE_API`, `RESOURCE_LIMIT`, `TIMEOUT`, `PLUGIN_STOPPED`, `INVALID_MESSAGE`, `CREDENTIAL_REQUIRED`, `NETWORK_DENIED`, `STORAGE_UNAVAILABLE`. Include a safe human-readable message; omit credentials and private source content.

## 5. Packages, identity, and author workflow

Package format: `.cmxaddon`, a gzip-compressed tar archive containing root `manifest.json`, root `plugin.js`, `README.md`, `LICENSE`, optional `NOTICE`, and optional `source.map` for developer diagnostics. No nested dependencies or install/uninstall scripts. Production ignores source maps unless the user explicitly enables developer diagnostics. Plugin identifiers use `publisher.name`, with both segments matching `[a-z][a-z0-9-]{1,39}`. Contribution and setting IDs match `[a-z][a-z0-9-]{0,39}`. Reject Windows reserved device names, trailing dots/spaces, and control characters in archive paths on every platform.

Example manifest, using the normative field names:

```json
{
  "format": "codemux.feature-plugin",
  "manifestVersion": 1,
  "id": "codemux.project-brief",
  "name": "Project Brief",
  "description": "Prepare project context for a chat draft.",
  "version": "1.0.0",
  "api": "^1.0.0",
  "entry": "plugin.js",
  "platforms": ["linux-x64", "windows-x64"],
  "author": {"name": "CodeMux", "url": "https://codemux.org"},
  "repository": "https://github.com/Zeus-Deus/codemux",
  "license": "MIT",
  "permissions": ["workspace.read", "git.read", "composer.append"],
  "http": [],
  "credentials": [],
  "contributes": {
    "commands": [{"id": "open", "title": "Open Project Brief", "requiresWorkspace": true}],
    "panels": [{"id": "brief", "title": "Project Brief", "icon": "file-text"}],
    "composerActions": [{"id": "insert", "title": "Add project brief", "icon": "file-text"}],
    "composerViews": []
  },
  "settings": [{"id": "includeFiles", "type": "boolean", "label": "Include changed filenames", "default": true}]
}
```

Validate with strict Rust serde types and a generated JSON Schema included in the SDK. Reject unknown fields/capabilities, duplicate IDs/settings, invalid versions, non-HTTPS metadata URLs, unrecognized icons, missing declared entrypoints, and incompatible API/platform values before execution. Use semver for versions and API ranges. Set contribution limits at 20 commands, eight panels, eight composer actions, and four composer views. Required manifest fields are those shown; contribution arrays, HTTP, credentials, and settings may be empty but must be present. Description ≤240 characters; titles ≤80; total manifest ≤64 KiB.

Settings types are boolean, string, integer, and enum. Strings are at most 4 KiB and default to empty; integer fields require min/max; enums require 1–50 values and a member default. Up to 50 settings. No regex patterns, arbitrary JSON Schema evaluation, executable validators, or HTML descriptions.

Archive guardrails: ≤10 MiB compressed, ≤30 MiB expanded, ≤64 entries, `plugin.js` ≤5 MiB. Reject traversal, absolute paths, drive letters, backslashes, symlinks, hardlinks, devices, duplicate/case-colliding paths, and unexpected entries. Extract into a fresh private staging directory with fixed modes, no archive-supplied permissions, and no following links. Streaming expansion must stop at the limit. Do not extract over any installed package. Legacy theme/footer/Wasm prototype manifests are explicitly unsupported.

Author commands to implement: `codemux-plugin init`, `build`, `check`, `pack`, `dev`. The starter has pinned dependencies, sample permissions, strict types, and a one-command build. `dev` rebuilds and produces a package; the app’s explicit Developer mode can watch one user-selected development package and reload after validation. Development is off by default; no directory auto-discovery, no package scripts in the app, and no filesystem path may be activated by a website link. Editing permissions still requires a fresh grant.

Do not execute a plugin during catalog/build validation. Publisher identity is established by catalog review, not trusted from the manifest alone. A local file is labeled “Local / unverified”; it cannot impersonate a catalog-verified installation or replace the same ID without an explicit source-replacement review.

## 6. Permissions, HTTP, and credentials

Installation never implies activation. The review screen lists workspace/Git data access, composer insertion, external links, every network origin/method, and credential use. An “Install & enable” action may combine the two explicit choices; importing without enabling leaves the package inert. Revoke a grant by disabling the package and reviewing reduced permissions before re-enabling; do not invent partial runtime behavior for missing required grants.

Bind grants to installation ID, plugin ID, source identity, normalized capability set, and the accepted release. Updates within the same source may reuse unchanged grants after the user chooses Update. Added origins/methods/credentials/permissions require new review. Removing access is immediate. A manifest’s author-supplied text never grants authority.

HTTP declarations use `{ "origin": "https://api.example.com", "methods": ["GET"], "credential": null }`. Origins must be exact HTTPS hostnames on port 443; no wildcards, IP literals, URL credentials, path prefixes, or fragments. Credential declarations use `{ "id": "apiToken", "label": "API token", "origin": "https://api.example.com", "type": "bearer" }` and the matching HTTP entry references that ID. Allow one HTTP declaration and at most one credential per origin in v1. Restrict credential-bearing operations to the declared origin. HTTP methods may include GET, POST, PUT, PATCH, DELETE; review explicitly describes possible external writes.

`http.fetch` takes the bound context, declared origin, absolute-path URL suffix starting with one `/`, method, optional UTF-8 body, and safe request headers. The broker reconstructs and revalidates the final URL. Reject `//`, scheme changes, traversal into another authority, Host/Cookie/Authorization/Proxy-* headers, CR/LF, and redirects. Limit request body to 256 KiB and decoded response body to 512 KiB so the complete result fits the 1 MiB frame. Limit aggregate plugin network traffic to 10 MiB/minute and four concurrent requests. Abort on disable/context disposal.

Use a dedicated reqwest client, separate from authenticated CodeMux services: no cookie jar, proxy inheritance, credential inheritance, or automatic redirects. Resolve and connect to validated public addresses only, preserving TLS hostname verification. Reject loopback, private, link-local, multicast, unspecified, carrier-grade NAT, and IPv4-mapped equivalents; prevent DNS rebinding by pinning the vetted resolution for the connection. No localhost/LAN APIs in v1. Return only content-type and explicitly safe rate-limit headers; never echo authentication headers. Check size incrementally, including decoded content. Default User-Agent identifies CodeMux and the plugin ID.

The manager collects bearer secrets in host-owned fields. Plugins can request use of a named credential but cannot read the token. Keep persisted secrets in the OS credential store using `keyring` 3.6.3: `windows-native` on Windows; `async-secret-service`, `tokio`, `crypto-rust` on Linux. Use an installation-specific service namespace and serialize access. If no unlocked Secret Service is available, offer explicit “this session only” in-memory storage; never fall back to plaintext files or the CodeMux account token. Never silently use keyring’s mock backend in production.

Uninstall deletes credential entries even when private non-secret data is kept. If deletion is temporarily unavailable, save an inaccessible cleanup tombstone and show a retryable warning; the removed plugin remains disabled and cannot use it. Logs, diagnostics, and exports redact secret values and request/response bodies. Data sent to a granted external service cannot be undone by uninstalling; say so in the permission review without claiming core corruption.

## 7. Storage, updates, removal, and recovery

Use `dirs::data_dir()/APP_DIR_NAME/addons-v1/`, honoring the existing development vs production app directory distinction. Keep a separate SQLite database `registry.sqlite`; do not extend the account-sync schema or make core database startup depend on plugins. Registry corruption opens the base app with plugins paused and an actionable manager error.

Directory layout:

```text
addons-v1/
  registry.sqlite
  packages/<plugin-id>/<sha256>/...
  state/<installation-id>/<data-generation>/state.sqlite
  staging/<random-id>/...
  recovery/<transaction-id>/journal.json
```

Logical registry records: installations (ID, plugin ID, source, active digest/version, desired enablement, state, data generation), accepted grants, accepted releases, update transactions, last failure, crash quarantine, catalog cache/blocklist, credential cleanup tombstones. All writes go through one manager and a per-plugin operation lock. Package content is immutable after validation; compare actual digest on activation and refuse modified content. The source tuple is catalog identity + publisher/repository, or local installation identity; a replacement is an explicit reinstall/review operation.

Private storage is JSON key/value with global and per-workspace scopes. Keys ≤128 ASCII characters, values ≤64 KiB, total ≤5 MiB/plugin. Use transactions and bind the namespace in the broker; no plugin-selected filenames or SQL. Uninstall defaults to removing private state, with an explicit “Keep data for reinstall” option. Retained data is orphaned under the prior installation identity and may be reclaimed only after matching-source confirmation, never by another package with the same display name.

Lifecycle states: `installed-disabled`, `activating`, `enabled-idle`, `enabled-running`, `updating`, `failed-disabled`, `incompatible-disabled`, `blocked-disabled`, `removing`. Persist desired enablement separately. No auto-restart after a runtime fault: show Retry/Disable/Remove. Retry creates a new generation only after a user action.

### Transactional install/update

1. Fetch or import into staging. Validate archive, schema, SHA-256, compatibility, source, and blocklist. No plugin execution.
2. Present the exact version and grant changes. Keep the current version active until the user accepts the update.
3. Lock the installation; stop old work, revoke its handles, and snapshot private state using SQLite backup. Start a candidate data generation from that snapshot. Record durable intent before switching anything.
4. Probe candidate activation with storage scoped to its candidate generation and external writes/composer mutations denied. Verify declared callbacks and readiness. Activation hooks must not perform external effects; reject attempts.
5. Atomically switch package digest, grants, and data generation in one registry transaction. Start the new version normally and keep the previous package/data snapshot for one rollback. If activation fails, restore the old tuple and its prior enablement; show the candidate failure.
6. On next startup, finish or roll back incomplete transaction journals before activating plugins. Fail safely when disk is full, validation fails, or the app is interrupted. Never run two generations of the same installation simultaneously.

An install by ID selects the highest compatible stable semver; an explicit version link must resolve exactly or fail. Do not silently substitute another release. Exclude prereleases unless explicitly imported as a local development package. Refuse equal-version/different-digest replacements and downgrades through Update; use the recorded rollback path for a prior accepted release. Normal updates are manual in v1. Automatic background checking can show an update badge, but must not activate new code. Rollback restores both the previous release and its matching data snapshot after warning that plugin-private changes since the update will be lost. It does not undo already completed external actions. For v1 there is no automatic storage migration hook; plugins must read older stored values defensively. A breaking private format needs a new schema key plus explicit author-managed conversion in the candidate generation, limited to private storage.

Disable first revokes broker access and generation tokens, rejects pending calls, cancels tasks, removes commands/accessories/panels, and flushes accepted storage writes. Give cleanup 500 ms, then kill/reap the child within the overall 2 s deadline. Host cleanup does not depend on plugin cooperation. Uninstall uses the same sequence, commits a removal tombstone, then deletes package/state data. If deletion fails, remain removed/inert and retry cleanup; do not resurrect the plugin on restart.

Add `CODEMUX_DISABLE_ADDONS=1`, a `--disable-addons` desktop launch option, and a Settings “Pause all add-ons” action independent of runtime health. Keep a session journal marking unclean activation; if the app exits uncleanly before its plugin startup checkpoint, next launch pauses plugins and offers recovery. A plugin-host crash alone quarantines that plugin. App startup and account sign-in must never wait for plugin catalog/network traffic.

## 8. Settings and user experience

Add a permanent Add-ons section in Settings, available with chat GUI both on and off. Keep the initial workspace UI unchanged. The manager contains Installed and Browse tabs; a persistent pause-all control; Import package and Install from link/ID; optional Developer mode.

Each installed row shows name, author/source verification, version, enabled/disabled/failure state, update availability, and Configure / Permissions / Disable / Remove. A detail view holds normal settings, host-owned credential fields, compatibility explanation, bounded diagnostics, and rollback where available. Do not show themed mock packages or footer presets as examples.

Empty state explains what add-ons do and offers Browse / Import. Offline state retains installed functionality and cached catalog with a freshness label. Unsupported platform, remote workspace, expired interaction, missing credential, and incompatible API each have a distinct message. Website summaries and READMEs are sanitized plain text/Markdown. Installation review must expose actual parsed metadata, not just catalog marketing text.

Web remote: hide add-on contributions, show an explanatory disabled manager entry, and reject every `addon_*` command in `web_remote/dispatch.rs` before channel rewriting/invoke. Do not forward the plugin UI event stream to remote browser subscribers. For a desktop displaying a remote workspace, workspace-bound plugin surfaces are unavailable. These rules prevent accidentally authorizing access to the wrong machine.

## 9. Catalog, website, and publication

Use the existing `codemux-sitev2` project. Add `/addons` and `/addons/[id]`, with searchable listings, official/community attribution, supported API/platforms, exact releases, capabilities, source repository, author, README summary, and report/submission links. Add navigation and plugin author documentation. No payment, ratings, account uploads, or user-installed inventory database.

Canonical curated entries live in the public CodeMux repository under `catalog/addons/`, so community contributors can submit PRs without access to the website repository. This is a directory in the existing project, not a required new service/repository. Add schemas and a validation workflow there. After maintainer review, generate `catalog-v1.json` and publish it as a versioned catalog artifact from the app repository. The website consumes an explicitly pinned artifact and includes it as `/addons/catalog-v1.json`. Do not fetch a mutable GitHub branch on every page request.

Catalog envelope fields: `schemaVersion: 1`, monotonically increasing `revision`, `generatedAt`, `plugins`, and `blocked`. Each plugin has unique `id`, `publisher`, `tier` (official/community), repository URL, descriptions, and `releases`. Each release has immutable semver `version`, `api`, `platforms`, full `sourceCommit`, `downloadUrl`, SHA-256, compressed byte size, publication date, license, and normalized manifest capabilities. Blocklist entries identify a plugin ID or digest, reason, and date. Maintain exact digest/source ownership across versions. Never reuse a version with different bytes.

V1 packages are GitHub Release assets from the declared public source repository. The installer accepts HTTPS GitHub release asset URLs and follows only GitHub’s required HTTPS asset redirects to the documented release asset hostnames, with public-IP checks and a maximum of three hops. Hash the final archive; a URL is not its identity. This download client is separate from plugin HTTP grants. No raw Git clone, branch install, arbitrary marketplace host, or build-on-install. Local import remains available and labeled unverified.

Catalog submission checks validate schema, unique identity/version, source ownership evidence, release digest/size, static archive contents, declared permissions, supported API/platforms, and license. Maintainers review source changes, capabilities, and release provenance before merging; catalog checks never execute submitted code. Updates receive a new reviewed entry. A dependency or capability change is part of that review. A listing is a review signal, not a guarantee against all defects.

The desktop fetches `https://codemux.org/addons/catalog-v1.json`, max 2 MiB, off the startup critical path. Cache successful snapshots; retain the highest accepted revision and reject rollback to lower revisions. Recheck at most once per 24 h unless the user chooses Refresh. Previously enabled packages work offline against the last known blocklist. Online installs and updates require a successfully refreshed catalog in that operation; do not silently bypass a fetch failure using stale trust metadata. A newly blocked package is disabled with a reason. A local import matching a known blocked ID or digest is also refused. Document that offline machines cannot learn new revocations until they reconnect.

Website installation handoff in v1 is explicit: “Copy install link” for `https://codemux.org/addons/<id>?version=<semver>`, plus Download package. Settings accepts that exact-domain link or catalog ID, looks up the release, and opens review. No OS protocol registration or browser-to-desktop localhost bridge in this release. This avoids introducing a second installation authority. A later deep link may reuse this same review flow.

Future account sync would store only selected IDs/versions/preferences, with separate device downloads and grants. It is not part of this implementation and requires no server/account migration now.

## 10. Code map and ordered pull requests

The following paths are relative to each repository. Existing paths are integration points; new paths are prescribed modules. Keep CodeMux’s current user changes intact and work on an isolated branch/worktree. Do not cherry-pick the old prototype wholesale.

| PR | Deliverable and main files | Depends on | Exit condition |
| --- | --- | --- | --- |
| 1 | Contracts and standalone host: new `src-tauri/addon-host/`, `src-tauri/addon-protocol/`, `packages/plugin-sdk/`, `packages/plugin-cli/`; contract fixtures and schemas | None | Repeat the included probe; implement framing/generation/resource limits; host builds and runs on both release OSes |
| 2 | Manager and broker: new `src-tauri/src/addons/{mod,manager,protocol,permissions,workspace,git,storage,http,credentials}.rs`, `commands/addons.rs`; wire `lib.rs` command registration | 1 | Bound host operations work; access/context/race and cancellation tests pass; no arbitrary internal invoke |
| 3 | UI integration: new `src/lib/addons/`, `src/stores/addons-store.ts`, `src/components/addons/`; extend `right-panel.tsx`, `right-panel/pane-registry.ts`, `pane-picker.tsx`, `pane-tab-strip.tsx`, `src/stores/ui-store.ts`, `overlays/command-palette.tsx`, `Composer.tsx`, `ComposerFooter.tsx` | 2 | Project Brief package drives a real panel, command, and composer action through the SDK; no core-only visual regression |
| 4 | Lifecycle manager UX: `settings/settings-view.tsx`, `src/lib/settings-sections.ts`, new `settings/addons-settings.tsx`; installer/update/recovery modules; `src/tauri/{commands,types,events}.ts`, `src/dev/tauri-mock.ts`, desktop startup/CLI flags, `web_remote/dispatch.rs` guard | 2, 3 | Install/disable/update/rollback/remove/restart behavior passes the lifecycle and remote tests |
| 5 | Independent examples and packaging: new `examples/addons/project-brief/`, `examples/addons/issue-companion/`, SDK docs, host build script, Tauri resource config, `.github/workflows/release.yml` | 1–4 | Both packages install without rebuilding CodeMux; release bundles include the correct host; packaged native smoke tests pass |
| 6 | Catalog pipeline: new `catalog/addons/`, schemas, generator and validator; workflow to publish reviewed catalog artifacts | 5 | Valid entries round-trip; invalid/blocked/tampered releases fail without executing them |
| 7 | Website: `app/addons/page.tsx`, `app/addons/[id]/page.tsx`, catalog loader/data, `components/nav.tsx`, `content/docs/addons*.mdx`, `content/docs/meta.json` | 6 | Site serves a pinned catalog; shared link resolves to the same digest and desktop review; source submission is documented |
| 8 | Release hardening and full handoff documentation | 1–7 | Acceptance matrix below passes; no reduced substitute for missing capabilities; publish/release only through normal authorized workflows |

Other relevant existing files: `src/components/chat/AgentChatPane.tsx`, `DraftChatSurface.tsx`, `src/stores/agent-chat-store.ts`, `src-tauri/src/git.rs`, `database.rs`, `src-tauri/src/json_rpc_child/mod.rs`, `src/App.tsx`, and `scripts/build-codemux-remote.sh`. The existing standalone-binary staging pattern is useful, but the new host must have an independent build without the main Tauri crate’s bootstrap dependency.

Linux host target: `x86_64-unknown-linux-gnu`, built on the existing Ubuntu 22.04 baseline. Windows host target: `x86_64-pc-windows-gnu`, bundled beside the MSVC-built app and communicating only by JSON. rquickjs lists Windows MSVC QuickJS support as experimental; use its tested GNU path for this standalone executable. CI installs the explicit Rust GNU target and MinGW C toolchain. Resolve host filenames by app platform/architecture; do not assume the host and app have identical Rust target strings. Include required runtime DLLs if linking produces them, or configure a self-contained host and verify on a clean Windows runner. No system Node, compiler, or Cargo may be required by end users.

## 11. Required example plugins

### Project Brief

An independently built `codemux.project-brief` package declares one command, one panel, and one composer action. Its panel shows active project name, branch or detached state, counts, a bounded list of changed paths, Refresh, and Add to draft. It handles non-Git/empty/remote workspaces, project switching, and missing composer. Refresh obtains live broker data. Add to draft inserts a short text brief only after a real click; preserve existing input. Its “include filenames” setting and private preference survive restart and obey uninstall data choices.

### Issue Companion

An independently built `codemux.issue-companion` package exercises the same API without new privileged exceptions. Configure repository owner/name and optional GitHub bearer credential through host settings. Use declared GET access to `https://api.github.com`; render an issue list with loading, empty, rate-limit, and authentication states. Opening a selected issue uses an attributed external link; adding its title/URL to a draft is explicit and preserves draft content. Do not automatically upload project data. CI uses recorded synthetic fixtures through the broker transport seam, not live API tokens. Optional manual validation uses a user-owned test repository.

Both examples are fixtures/distribution packages, not bundled privileged React components. They must exercise the same manifest, grants, renderer, process host, and installer as third-party packages. Neither is installed/enabled automatically for existing users.

## 12. Acceptance matrix and verification

Implement behavior-driven tests around the following cases. Do not replace native isolation/IPC checks with the Tauri mock. Add test seams for clocks, downloader, credential store, filesystem root, workspace state, and process supervision; test seams must not become public bypasses.

| Area | Required evidence |
| --- | --- |
| Core independence | Clean install and pause-all launch preserve chat, terminal, projects, theme settings, and official update flow; zero plugin hosts until needed |
| Functional SDK | Both example packages install on the stock built app and perform their documented project/composer/network operations |
| Author independence | Build one example from a directory outside the app checkout using a packed/published SDK and its documentation only |
| Runtime failure | Throw, infinite loop, endless promise chain, recursive stack overflow, excessive allocation, unexpected child exit, and ignored shutdown quarantine one plugin and leave the app interactive |
| UI hostility | Unknown tag, HTML/script prop, URL/CSS injection, malformed/cyclic IDs, stale callbacks, mutation flood, oversized frame/tree, and deep nesting are rejected before host rendering |
| Authority | Forged plugin/workspace identity, undeclared method, denied capability, wrong generation, and expired interaction cannot invoke host operations |
| Context races | Switch project, close thread, replace draft surface, type during a delayed plugin request, disable during HTTP/Git, and uninstall during activation; no late insertion or cross-project leak |
| Package validation | Traversal, links, Windows reserved names, archive bombs, duplicate paths, mismatched digest, unsupported manifest/API/platform, and tampered package fail inertly |
| Network | Private/mapped IP, DNS rebinding, redirects, credential-header injection, excessive/decoded body, timeout, and disallowed method/origin fail; approved public request succeeds |
| Secrets | Correct keyring backend, locked/missing service, explicit session-only fallback, per-source isolation, redaction, uninstall deletion, and retryable cleanup tombstone |
| Persistence | Restart keeps grants/settings; retained data cannot be claimed by another source; registry corruption pauses plugins without blocking core startup |
| Updates | Same-permission update, expanded-access review, cancel, interrupted stage/commit, full disk, candidate failure, rollback with matching private data, and blocklist disable |
| UI and access | Keyboard operation, screen-reader labels, small windows, dark/light themes, chat GUI off, core pane restoration, no empty accessory/footer spacing without plugins |
| Remote boundary | Browser clients cannot invoke `addon_*` or receive plugin events; remote workspaces cannot accidentally use local Git or composer handles |
| Website/catalog | Search/detail/download/link, schema generation, duplicate versions, source mismatch, catalog rollback, stale cache, offline behavior, and reviewed revocation |
| Packaged platforms | Linux AppImage/deb and Windows NSIS smoke tests launch bundled hosts with correct limits; no development-directory dependency |

While iterating, follow repository instructions: `npm run check` and only affected `npm run test -- <file>`; `cargo check -j 2 --manifest-path src-tauri/Cargo.toml`; focused `cargo test -j 2 --manifest-path src-tauri/Cargo.toml <filter> -- --test-threads=2`. Test standalone crates with their own manifests and `-j 2`. CI owns full suites.

For app UI changes, use `npm run dev` and the prescribed CodeMux browser when `CODEMUX` is set; capture before/after screenshots with synthetic data. Use `npm run tauri:dev` or packaged builds for actual native IPC/runtime verification. Stop servers/processes you started. Website verification: affected Vitest tests plus `npm run build` (the existing lint script is not evidence of a working Next.js lint command). Rebase before PRs; follow each repository’s current instructions.

Release gates: all matrix cases pass on supported platforms; core tasks remain operable during hostile-plugin cases; a plugin fault is contained within 2 s; idle plugins do no continuous polling; UI updates stay within measured budgets. Document hardware and workloads with performance results rather than claiming a universal latency number. Any failed gate means the feature stays unreleased, not that the gate is quietly removed.

## 13. Agent handoff instruction

> Implement the CodeMux feature plugin platform described in this specification, starting from the current CodeMux and codemux-sitev2 codebases. Treat the scope, execution boundary, SDK operations, lifecycle semantics, and acceptance matrix as requirements. Work through the ordered PR deliverables; preserve existing user changes and core behavior. Use Hermes for contribution/lifecycle patterns, but do not load plugin code in CodeMux’s main renderer. Build and verify the two independent example plugins. Do not substitute theme packages, footer presets, mock-only UI, or standalone checklist panels for the feature platform. The included research probe is evidence and a starting fixture, not production code. Implement remaining validation gates and report results honestly. Resolve ordinary implementation details autonomously; only revisit a specified design decision if code or test evidence demonstrates a concrete incompatibility, and document the replacement and its effect before continuing dependent work. Follow normal repository requirements for tests, screenshots, PRs, and publication authorization.

Possible later SDK additions include sidebar navigation entries, full-page views, richer composer controls, and further project operations. They require documented scopes and cleanup tests; they do not require a fork or a redesign of this plugin model.

No product questions are pending. The main architectural choices are made. Implementation validation remains work to perform, as it does for any engineering specification; passing a probe is not a substitute for the release gates.

## 14. Sources and evidence

- [Hermes native Desktop SDK](https://hermes-agent.nousresearch.com/docs/developer-guide/desktop-plugin-sdk) — contribution and authoring model.
- [Hermes runtime loader](https://github.com/NousResearch/hermes-agent/blob/498abb677e/apps/desktop/src/contrib/runtime-loader.ts) — execution-boundary distinction; inspected locally.
- [Hermes catalog policy](https://github.com/NousResearch/hermes-agent/blob/498abb677e/plugin-catalog/README.md) and [catalog](https://hermes-agent.nousresearch.com/docs/plugins) — reviewed, separately distributed releases.
- [Remote DOM](https://github.com/Shopify/remote-dom) — rendering across an execution boundary; inspect its pinned package documentation for adapters.
- [rquickjs runtime API](https://docs.rs/rquickjs/latest/rquickjs/struct.Runtime.html) and [platform support](https://github.com/DelSkayn/rquickjs) — memory/interrupt controls and native target limitations; exact 0.13.0 sources were used locally.
- [Tauri capabilities](https://v2.tauri.app/security/capabilities/) — app permissions are not a substitute for the plugin broker.
- [reqwest 0.12.28 client configuration](https://docs.rs/reqwest/0.12.28/reqwest/struct.ClientBuilder.html) — existing HTTP dependency; use a dedicated constrained client.
- [keyring 3.6.3](https://docs.rs/keyring/3.6.3/keyring/) — explicit backend features and credential-store failure handling.
- [Runtime probe source](research-probe/src/main.rs), [UI probe](research-probe/probe-ui.js), [results](research-probe/probe-results.json), [reproduction](research-probe/README.md).

All architecture limits, SDK contracts, PR boundaries, and release policies above are decisions for CodeMux. They are not claims that Hermes already implements the same isolation or that CodeMux’s full platform was tested during this planning task.
