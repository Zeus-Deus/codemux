# Plugin system: decisions the build spec left open

One line each: decision — why — how to revert.

- A Local plugin (a folder with no install marker) starts **on**; catalog and Git installs start off — you put a Local folder there yourself, and drop-in development needs it to run without a trip to Settings — make `defaultEnabledOf` in `src/contrib/runtime-loader.ts` return `false`.
- One recursive `notify` watcher on the plugins root, debounced 150 ms, emits `plugins_changed { id }` per changed folder (not per-file watches) — one watcher covers new folders, deletes and saves, and dot-folders (`.staging-*`) are ignored so installs don't flicker — replace `plugins/watch.rs` with per-entry watches.
- A registration's disposer removes that exact registration, not "whatever has this id now" — otherwise a hot reload's late cleanup could delete its own replacement — compare by `id` in `ContributionRegistry.take`.
- `plugins_read_source` takes a plugin id, never a path, and re-checks containment with canonical paths — the renderer can't name arbitrary files through it — n/a (security floor).
- A linked plugin folder (`plugins/<id>` is a symlink to a checkout) is allowed; its `plugin.js` must sit directly in the link target — supports developing a plugin in its own repo — refuse symlinked folders in `paths::contained_entry`.
- The plugin loader is a lazy chunk imported after the first React commit (`startPluginsAfterMount` in `src/main.tsx`) — first paint never waits on plugin I/O and a plugin-free start pays one small import — call `startPlugins()` inline.
- `minCodemux` is compared as three numeric dotted parts; pre-release tags are ignored — enough for a gate, no semver dependency — swap `satisfiesMinVersion` for a semver library.
- Plugin errors toast once per failed load/registration and stay on the plugin's row until the next good load — attribution without toast spam on every save — n/a.
- Windows (WebView2) was not tested for blob-module imports in milestone 1: no Windows runner or VM was available — record only; test on Windows before release.
- One pull request, updated per milestone, kept as a draft until milestone 7 — milestones build on each other and this repo's CI only runs for PRs that target main, so stacked PRs would show no checks — split by cherry-picking each milestone's commits onto its own branch.
- The `src/plugins/**` import fence is a Vitest test (`src/plugins/import-fence.test.ts`), not an ESLint rule — Codemux has no ESLint, and adding a linter for one rule is more weight than the rule; the test runs in CI with every other test — replace it with Hermes's `no-restricted-imports` block if ESLint is ever adopted.
- Bundled plugins split into `meta.ts` (eager, tiny) and `plugin.ts(x)` (lazy chunk) — the spec's "never imported when off" needs the code out of the main bundle while Settings still lists the row — merge into one eager `plugin.tsx` (Hermes's shape) and accept that off plugins ship in the main chunk.
- The kill switch still lists bundled rows (no code loaded) so Settings can show what is being held back — a blank page reads like a bug — skip `adoptBundledPlugins()` when `disabledByEnv`.
- Safe mode pauses only non-bundled plugins, matching the spec; bundled plugins follow their saved decision — they ship with the app and are reviewed with it — add `|| kind === "bundled"` to the pause check to pause everything.
- The 10-second clean-start window starts when plugin loading begins, not at process start — that is the moment a plugin could take the app down — move the timer into `main.tsx` before `mountApp()`.
