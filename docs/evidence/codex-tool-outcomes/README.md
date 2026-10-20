# Codex tool outcome replay

Captured on 2026-10-20 in the Chromium collaborative preview at 1280×800,
using `npm run dev` with the Tauri mock and synthetic data only. These are
browser/reducer checks, not native WebKit, real IPC, or physical-input checks.

Both screenshots hydrate the same transcript through the actual app store:

- `dynamicToolCall`: `status: "failed"`, `success: false`.
- `mcpToolCall`: `status: "completed"`, non-null `error.message`.
- `fileChange`: `status: "inProgress"`, with no terminal outcome evidence.

Each canonical `tool_result` deliberately has the historical `is_error: false`.
A successful `turn_completed` follows the three results. Opening the work log
shows the outcome summary.

- **Before:** main's reducer produces `done`, `done`, `done`; no failure count.
- **After:** the fixed reducer produces `error`, `error`, `unconfirmed`; the
  work log shows two failures and the patch's unconfirmed outcome.

The before capture temporarily used the unchanged reducer from main, then the
fixed source was restored before the after capture. Stored user histories and
native app processes were not used or modified. The dev server was stopped.

The shared regression fixtures in
`src-tauri/src/agent_provider/codex/fixtures/tool-outcomes.json` cover the same
evidence rules through Rust adapter translation and frontend live/replay paths,
including result-before-use ordering and bounded metadata for lazy history.

Protocol fields/statuses were checked against the installed `codex-cli 0.162.1`
using `codex app-server generate-ts --experimental`. Commands and patches expose
`inProgress | completed | failed | declined`; dynamic and MCP calls expose
`inProgress | completed | failed`. Codex maps approval cancellation and rejection
to `declined`, so the adapter does not invent a separate cancellation status.
Legacy stdout-only command results keep their saved canonical error flag.
