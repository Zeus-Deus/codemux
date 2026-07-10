# Dev Mock Runtime

- Purpose: Describe the dev-only Tauri runtime shim that boots the Codemux UI in a plain browser with no desktop window.
- Audience: Anyone doing UI/visual work, screenshots, or browser-driven verification.
- Authority: Canonical feature-level reality doc for the `src/dev/` mock harness.
- Update when: The mock's command surface, guards, seed fixtures, or load path change.
- Read next: `docs/core/TESTING.md`, `docs/features/workspaces-overview.md`

## What This Feature Is

A dev-only shim that mocks the Tauri runtime so the real Codemux React UI can boot in an ordinary browser tab under `npm run dev` (Vite at `http://localhost:1420`), with no desktop window and no Rust backend. It exists so visual/component work — and the Codemux browser-pane screenshot workflow — can iterate against the actual UI loaded with realistic seed data, instead of needing a full `tauri:dev` desktop build.

## Current Model

`main.tsx` conditionally installs the shim before React mounts:

```ts
if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
  await import("./dev/tauri-mock");
}
```

Two guards, both required:

- `import.meta.env.DEV` is statically `false` in a production build, so Rollup drops the dynamic-import branch entirely — the mock is never in the shipped bundle (`grep -r "tauri-mock" dist/` is empty after `npm run build`).
- The runtime `__TAURI_INTERNALS__` check keeps the shim dormant under `npm run tauri:dev`, where the real WebView already injected `window.__TAURI_INTERNALS__`. Real-IPC behavior is byte-identical to before the mock existed.

The shim installs a faithful-enough `window.__TAURI_INTERNALS__` (plus the event-plugin internals) so the unmodified `@tauri-apps/api` code — `invoke()`, `listen`/`emit`/`once`, the window/app plugins, and the `plugin-opener` / `plugin-updater` / `plugin-process` / `plugin-dialog` wrappers — routes every call into an in-process command router. No module aliasing is needed.

Command handling:

- Boot-critical reads return hand-curated fixtures from `mock-fixtures.ts`.
- Mutators update an in-memory `AppStateSnapshot` and re-emit `app-state-changed`, so the UI reflects the change just as it would against the real backend.
- Everything else falls through to a logged, shape-safe default.

`mock-fixtures.ts` mirrors `src/tauri/types.ts` exactly, so TypeScript catches drift the moment a real snapshot field changes. The seed (per issue #40 acceptance criteria) covers three projects, primary + worktree workspaces, all four PR states, agent states (working / review / permission), a linked issue, a muted workspace, non-zero git ahead/behind/diff stats, and a non-zero notification count.

## What Works Today

- Boots the full UI in a browser tab via `npm run dev` with no desktop window or Rust backend
- Realistic seed data so the sidebar, workspaces overview, tabs, and status dots render populated
- Mutations re-emit `app-state-changed`, so interactive flows update live
- Compatible with the Codemux browser commands (`codemux browser open http://localhost:1420`, `snapshot`, `screenshot`) for visual proof
- Tree-shaken out of production builds; dormant under `npm run tauri:dev`
- **Browser pane exercisable end-to-end**: the seeded `browser-demo`
  workspace hosts a `browser` pane (id `MOCK_BROWSER_ID`), and the
  mock's `start_browser_stream` handler returns `ws://127.0.0.1:9777`.
  Spawn a real daemon on that port first —
  `AGENT_BROWSER_STREAM_PORT=9777 agent-browser open <url> --headless --session devmock`
  — and the pane streams real frames with full input forwarding
  (drag-select, hover, cursor probe, clipboard bridge) in plain-browser
  dev. Without a daemon the pane shows its connecting/retry state,
  identical to a dead daemon in the real app. (`agent_browser_run` is a
  mock no-op, so toolbar navigation/viewport calls are accepted and
  ignored — navigate the daemon via the `agent-browser` CLI instead.)
- **Agent-chat mocked end-to-end** (`enable_agent_chat` is ON in the
  mock flags; issues #75/#77). Two seeded workspaces cover both chat
  paths:
  - `agent-chat-demo` — its `agent_chat` pane is pre-bound to
    `MOCK_CHAT_THREAD_ID`; `agent_chat_list_messages` hydrates a long
    generated transcript so the virtualized MessageList is exercisable,
    and `window.__codemuxChatMock.streamReply()` triggers a live
    streamed reply on demand (`{ emptyGapTicks: N }` reproduces the
    issue #155 stream shape — an empty first text delta then N silent
    ticks before prose — for verifying the working indicator never
    drops out mid-turn).
  - `chat-streaming` — its pane has no thread bound, walking the
    fresh-session flow: `agent_chat_start_session`, then the
    per-thread channel pair
    `attach_agent_chat_output` / `detach_agent_chat_output`.

  Live replies stream token-by-token `content_delta` frames through
  the registered `@tauri-apps/api` `Channel` — same ordered
  `{ index, message }` dispatch the real IPC layer uses — followed by
  `item_completed` / `turn_completed` / `session_state_changed`,
  mirroring the backend's `forward_event` channel routing.
- **Terminal scrollback serialize/restore mocked end-to-end**
  (issue #128). The mock is a faithful two-store twin of
  `src-tauri/src/scrollback.rs`: a session-keyed in-memory **cache**
  (`cache_terminal_scrollback` puts on unmount,
  `uncache_terminal_scrollback` removes on the next mount) and a
  `(workspace, pane)`-keyed **"disk"** store written by
  `save_terminal_scrollback` (live panes on close) and by
  `flush_scrollback_cache` draining the cache; `get_terminal_scrollback`
  reads **only** the disk store, matching the backend. Every handler
  emits a `[mock::scrollback]` console line, so the
  close → flush → disk → restore dance is observable in a plain browser.
  PTY output channels are tracked per session (ordered `{ index, message }`
  frames, index sequential per attach), and
  `window.__codemuxTerminalMock` exposes `flood()` (150k numbered lines
  ending `MOCK-FLOOD-END <n>`, chunked across ticks like a real PTY) and
  `emitSerializeBuffers()` (fires the app-close `serialize-terminal-buffers`
  event). Both are also bound to `Ctrl+Alt+Shift+F` / `Ctrl+Alt+Shift+S`
  so the CLI-driven browser (`codemux browser key-press`, no JS eval) can
  drive the serialize/restore e2e — flood a pane, switch, and assert the
  idle-serializer reuse vs. dirty-fallback behavior.
- **Draft surface / Thread Scope exercisable**
  (`enable_lazy_workspace_creation` is ON in the mock flags): "New
  thread"/"New agent" render the draft composer with the below-composer
  `ThreadScopeRow`, and a `list_branches_detailed` mock returns a
  recency-sorted branch list whose names overlap the seeded codemux
  worktree workspaces — so the "from ⑂ branch" picker's All/Worktrees
  tabs, WORKTREE badges, kind icons, and ages all light up in dev.

## Current Constraints

- Dev-only — never loads in production or under real Tauri IPC
- Not a backend substitute: handlers are fixtures + in-memory state, not real terminal/git/agent behavior. Terminals, real PTY output, git operations, agent sessions, and anything needing the Rust runtime are not exercised here.
- The terminal scrollback stores are in-memory twins of `scrollback.rs` (no real file I/O to `~/.local/share/codemux/scrollback/`, no real xterm serialize behind them). They exercise the frontend cache/flush/restore wiring and its observability, not a real PTY — `flood()` pushes synthetic frames through the output channel, it does not spawn a shell.
- The command surface was enumerated once at implementation time; commands added later fall through to the logged default until added to the router
- For real-IPC behavior use `npm run tauri:dev` (desktop window, not visible in the Codemux browser pane)

## Important Touch Points

- `src/dev/tauri-mock.ts` — the runtime shim + command router + event subsystem
- `src/dev/mock-fixtures.ts` — hand-curated seed `AppStateSnapshot` and fixtures
- `src/main.tsx` — the dual-guarded conditional import
- `docs/core/TESTING.md` — visual-verification workflow that uses this mock

## Notes

- Keep `mock-fixtures.ts` shapes aligned with `src/tauri/types.ts`; TypeScript is the drift guard.
- When a new boot-critical command appears, add a handler to the router so the UI doesn't fall through to the shape-safe default.
