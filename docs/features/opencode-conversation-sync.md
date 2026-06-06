# OpenCode Conversation Sync (cloud-push, issue #16)

- Purpose: Describe how a pushed/pulled workspace carries its **OpenCode**
  conversation between the laptop and a remote host, so `opencode` continues
  the same session on either side.
- Audience: Anyone touching the push/pull flows, the daemon-backed relaunch,
  the session adapters, or OpenCode integration.
- Authority: Canonical feature doc for OpenCode conversation continuity across
  cloud-push. Sibling of the Claude sync (see `docs/features/remote-hosts.md`).
- Update when: The OpenCode storage model, the export/import contract, the
  relaunch command, or the sync wiring changes.
- Read next: `docs/features/remote-hosts.md`, `docs/features/persistent-agents.md`

## What This Feature Is

PR #15 made **Claude Code** conversations follow a workspace across push/pull
(rsync the per-project JSONLs, relaunch with `claude --resume <uuid>`). This
feature does the equivalent for **OpenCode**: push a workspace with an active
OpenCode pane and the conversation continues on the host; pull it back and it
continues on the laptop. Verified monotonic across 3+ push/pull cycles.

## Why OpenCode is different from Claude

Claude stores per-project conversation JSONLs at
`~/.claude/projects/<encoded-cwd>/<uuid>.jsonl` — trivially rsync-able.

OpenCode stores **every** conversation in one SQLite DB at
`~/.local/share/opencode/opencode.db`. We can't rsync the file — that would
clobber the host's other OpenCode history. We must move just this workspace's
session and merge it into the receiving DB without touching anything else.

## Current Model — `opencode export` / `import`, not raw SQLite

The issue proposed hand-extracting this workspace's rows via `rusqlite` and
re-INSERTing them remotely. We deliberately **did not** do that. OpenCode's
schema is large, undocumented, and fast-moving (1.16.0 alone carries
`session` / `message` / `part` **plus** `session_message`, `event` /
`event_sequence` event-sourcing, `workspace`, `session_input`, `todo`,
`project_directory`, …). A bespoke transitive row extractor would be fragile
across OpenCode versions and laptop/host skew.

Instead we use OpenCode's **official, version-stable** CLI:

- `opencode export <sessionID>` → a portable JSON bundle
  (`{info:{id,directory,…}, messages:[{info,parts}]}`). Read-only.
- `opencode import <file>` → inserts/updates **just that one session**. It
  **preserves the session id**, is **idempotent** on re-import, and importing
  a longer (continued) bundle over a shorter one merges in the new messages.
- `opencode db "<sql>"` → headless query, used to find which session to export.

Two properties make this safe and correct:

1. **`import` derives the session's `directory` from its process cwd** (it
   ignores `info.directory` in the bundle). So we run `import` *from the
   workspace directory* on the receiving side, which associates the session
   with that cwd — and touches **only the one imported session id**, never the
   host's other sessions. This satisfies issue #16's "do NOT clobber unrelated
   sessions" criterion by construction (verified in the e2e test below).
2. **Importing a continued bundle merges**, so each push/pull cycle only grows
   or preserves the transcript — the monotonic property the acceptance criteria
   require.

## What Works Today

- **Push**: `ssh::opencode_db_sync::sync_opencode_session` finds the laptop's
  newest root session for the workspace cwd (`opencode db`), exports it
  (`opencode export`), uploads the bundle, and imports it on the host from the
  remote cwd. The synced session id is stashed in the pane's adapter captures.
- **Pull-back**: `pull_opencode_session` runs the symmetric flow — find the
  host's newest session for the remote cwd, export it there, download, import
  locally from the workspace cwd.
- **Relaunch**: `terminal::daemon_backed::build_agent_relaunch_command` builds
  `opencode --session <id>` when a session id was synced (else
  `opencode --continue`, which resumes the most-recent session for the cwd —
  exactly what `import` just made current). Generalized from the previously
  Claude-only synthesis; the gate that prevents the Linux preset-leak bug now
  treats an OpenCode session id as relaunch evidence too.
- **Local app-restart parity**: an `opencode` session adapter
  (`session_adapters.rs`, config v4) gives a local OpenCode pane `--continue`
  resume on app restart, mirroring the Claude adapter.
- **PATH-robust on the host**: every remote `opencode` invocation runs through
  a login shell fed over stdin (`ssh host bash -ls`), so `opencode` is found
  even when it lives under `~/.local/bin` / `~/.opencode/bin` (which
  non-interactive SSH wouldn't have on PATH).
- Best-effort throughout: a sync failure only loses continuity (the agent still
  launches), so it logs and continues — identical to the Claude sync.

## Current Constraints

- **Requires `opencode` on the host** (resolvable from a login shell). If it's
  missing, the import fails → logged, and the relaunch's binary preflight in
  `daemon_backed` surfaces an actionable "install it on the host" message.
- **One session per workspace direction** — the newest *root* session for the
  cwd. Child / sub-agent sessions (`parent_id IS NOT NULL`) are not bundled;
  `opencode export` only carries the target session's own messages. The visible
  TUI transcript is the root session, which transfers fully.
- **Directory match is exact**: the laptop-side lookup matches
  `session.directory = <workspace cwd>`. A session started in a sub-directory
  of the workspace won't be found (best-effort skip).
- Unix-only, like the rest of the cloud-push transport.

## Testing

- **Unit** (`ssh::opencode_db_sync::tests`, `terminal::daemon_backed::tests`,
  `session_adapters::tests`): SQL/marker parsing, script construction, and the
  per-agent relaunch command (`opencode --session <id>` vs `--continue`, claude
  unchanged, no cross-agent flag leakage).
- **End-to-end** (`src-tauri/tests/opencode_sync_roundtrip.rs`, env-gated):
  drives the real `sync_opencode_session` / `pull_opencode_session` over real
  SSH + real `opencode` against a Docker host stood up by
  `scripts/e2e/opencode-sync-e2e.sh`. Asserts: a session pushed to the host
  grows from short→full when continued there, the full transcript pulls back,
  3 push/pull cycles preserve it with no loss/dupes, and the host's unrelated
  session is byte-for-byte intact throughout. Skips cleanly when the
  `CMX_OC_E2E_*` env vars aren't set, so CI stays green without a host.

  Verified run: session grew 4→644 messages across a host continuation,
  preserved over 3 cycles; unrelated session held at 638 messages throughout.

- **End-to-end on a genuinely-run session** (`src-tauri/tests/opencode_real_roundtrip.rs`
  + `scripts/e2e/opencode-real-session-e2e.sh`, gated on `CMX_OC_E2E_REAL=1`):
  the strongest proof, on data created by *actually running* `opencode` (real
  model turns via a free `opencode/*-free` model, in an isolated-but-auth'd
  laptop DB that never touches the user's real one). It establishes a secret in
  a real session, pushes it with the real `sync_opencode_session`, then does a
  real `opencode --session <id>` turn **on the host** that must recall the
  secret — proving the pushed session genuinely resumes *with conversation
  context* on the remote — then pulls the continuation back. Verified: the host
  resume recalled the secret, the continuation returned to the laptop, and the
  host's unrelated session stayed intact.

## Important Touch Points

- `src-tauri/src/ssh/opencode_db_sync.rs` — the sync module (export/import/db +
  SSH transport + pure helpers).
- `src-tauri/src/ssh/mod.rs` — re-exports `sync_opencode_session` /
  `pull_opencode_session`.
- `src-tauri/src/commands/hosts.rs` — push (`workspace_push_to_host`) and pull
  (`workspace_pull_back`) call the sync alongside the Claude JSONL sync;
  `record_opencode_session_capture` stashes the synced id into the OpenCode
  pane's adapter captures.
- `src-tauri/src/terminal/daemon_backed.rs` — `build_agent_relaunch_command`
  (opencode + claude) and the generalized `should_synthesize_agent_relaunch`
  gate.
- `src-tauri/src/session_adapters.rs` — the `opencode` adapter (config v4).
- `src-tauri/tests/opencode_sync_roundtrip.rs`, `scripts/e2e/opencode-sync-e2e.sh`
  — the live round-trip harness.

## Notes

- The "active session id" needs no live capture (unlike Claude's hook): it's
  determined from the DB at sync time. The id is stable across resume (OpenCode
  appends to the same session unless `--fork`), so it stays constant across
  cycles.
