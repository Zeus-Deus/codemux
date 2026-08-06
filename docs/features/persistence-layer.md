# Local Persistence Layer (SQLite + JSON state)

- Purpose: Describe where Codemux stores durable local data, how the schema evolves, and the rules a change to either store must follow.
- Audience: Anyone adding a column, a table, or a persisted field — and anyone debugging data loss, upgrade, or downgrade behavior.
- Authority: Canonical feature-level reality for local storage and the migration model.
- Update when: The storage layout, the migration convention, or the split between SQLite and JSON changes.
- Read next: `docs/features/session-persistence.md`, `docs/features/settings-sync.md`, `docs/reference/ARCHITECTURE.md`

## What This Feature Is

Everything Codemux remembers between launches. It is deliberately **two stores with
two different evolution models**, and knowing which one owns a given piece of state is
the whole point of this doc:

- **SQLite** (`codemux.db`) — row-shaped, queryable, incrementally migrated records:
  chat threads and their messages, hosts, automations, the cross-device workspace
  registry, paired web-remote sessions, encrypted auth tokens, and free-form key/value
  namespaces.
- **JSON files** — whole-document state rewritten wholesale: the workspace/pane layout
  (`layout.json`), feature flags and the log ring (`observability.json`), the cached
  user-settings blob, and one sentinel file.

This doc covers the storage layer itself. What *user-visible* state survives a restart is
listed in `docs/reference/FEATURES.md` § Persistence; terminal scrollback and agent
session resume are their own subsystem in `docs/features/session-persistence.md`.

## Current Model

### There are two separate databases, both named `codemux.db`

**Desktop app** — `database_path()` (`src-tauri/src/database.rs`) resolves
`dirs::config_dir()/<APP_DIR_NAME>/codemux.db`. `APP_DIR_NAME` is build-mode dependent
(`src-tauri/src/lib.rs`): `codemux` in release, `codemux-dev` in debug, so a dev build
never touches the installed build's data. On Linux that is
`~/.config/codemux/codemux.db`.

Three separate processes open that same file: the desktop app, the `codemux`
CLI (`login`/`logout`/`status`, and the web-remote `connect`/`serve` paths), and the
`codemux-remote` **automation scheduler**, which deliberately uses the desktop DB rather
than its own.

**Headless remote daemon** — `codemux-remote serve` has its own database and its own
schema: `dirs::data_dir()/codemux-remote/codemux.db` (`src-tauri/src/remote/config.rs`),
holding a single `workspaces` registry table. Note it is **not** `APP_DIR_NAME`-scoped, so
a debug and a release daemon on one machine share one registry — the one place local
state is not build-separated.

If `init_database()` fails for any reason, `lib.rs` falls back to
`DatabaseStore::new_in_memory()` and the app runs with a volatile database, warning only
on stderr.

### Desktop tables

Device-local unless marked:

| Table | Holds |
| --- | --- |
| `schema_version` | One row with `SCHEMA_VERSION`. Write-only bookkeeping — see below. |
| `settings` | Free-form key/value. Backs presets (`preset_store`) and per-project scripts (`project.scripts:<root>`). Not the synced user-settings blob. |
| `ui_state` | Per-device UI state: sidebar width, collapsed-project flags, active workspace. |
| `recent_projects` | Recently opened project paths. |
| `auth_tokens` | Single row (`CHECK (id = 1)`) holding the AES-256-GCM-encrypted account session. |
| `agent_chat_sessions` | One row per chat thread: provider, workspace, cwd, title, `sdk_session_id`, and per-thread model / effort / context window / permission mode / fast mode. |
| `agent_chat_messages` | Append-only JSON event envelopes per thread (FK CASCADE). Its rowid is the frontend's `lastPersistedEventId` hydration cursor. |
| `agent_chat_checkpoints` | Run-start rollback bookkeeping: shadow git ref plus snapshot/head commit. |
| `hosts` | **Cloud-synced.** User-defined SSH targets — identity only; credentials stay in `~/.ssh/`. |
| `automations` | **Cloud-synced.** Scheduled agent runs: prompt, agent, RFC 5545 schedule, host, retention. |
| `automation_runs` | Per-device only, deliberately not synced. One row per fire, with a `UNIQUE(automation_id, scheduled_for)` idempotency key. |
| `workspaces_sync` | **Cloud-synced.** Cross-device workspace registry: identity and project metadata, no runtime state. |
| `web_remote_sessions` | Paired browser devices. Stores `token_hash` (SHA-256); the plaintext token is never persisted. |

Two tables, `projects` and `workspace_state`, are created on every install and **never
read or written** by any code path. They are vestigial.

Sync granularity is per-column, not just per-table: `workspaces_sync.default_branch` is a
local-only column inside a synced table — the server payload types omit it, so it survives
cloud pulls untouched.

### The migration model: idempotent replay, no versioning

There is no migration framework. `create_schema` re-runs in full on every open, in three
stages:

1. **Declarative creation.** One `execute_batch` of `CREATE TABLE IF NOT EXISTS` /
   `CREATE INDEX IF NOT EXISTS`. Table bodies are always kept at the *current* full shape,
   so a fresh install gets every column in one statement.
2. **An append-only ledger of additive `ALTER`s.** Because step 1 is a no-op on a database
   that already has the table, an existing install only gets a new column from an explicit
   `ALTER TABLE ... ADD COLUMN`. Each is idempotent by error-swallowing: on a database that
   already has the column SQLite returns `duplicate column name`, which the loop ignores;
   **any other error aborts schema setup**. The same array also carries destructive
   cleanups (`DROP TABLE IF EXISTS openflow_history`, reclaiming the retired orchestration
   feature's history) and idempotent data heals (the `permission_mode` NULL/cross-provider
   backfills), which are ordered *after* the `ALTER` they depend on.
3. **A version stamp** — `SCHEMA_VERSION` (currently **10**) is inserted or unconditionally
   overwritten.

**`SCHEMA_VERSION` gates nothing.** No production code path ever reads it back; the only
`SELECT version FROM schema_version` occurrences in the repo are two test assertions. On a
version mismatch nothing special happens in either direction — the same idempotent DDL is
replayed, and the stored number is overwritten with whatever the running binary was
compiled with, including *downward* when an older binary opens a newer database. There is
no down-migration path anywhere: no `DROP COLUMN`, no reverse list, no rollback.

The remote daemon mirrors this convention exactly, minus the version table.

### What lives in JSON instead

- **`layout.json`** (`~/.config/<APP_DIR_NAME>/layout.json`, sibling of the DB) — the whole
  `AppStateSnapshot`: workspaces, terminal sessions, browser sessions, notifications,
  `Review` pane statuses, **archived workspaces**, and config. Written pretty-printed in
  full on a 500 ms debounce, plus a synchronous flush on close. `persistable_snapshot`
  strips runtime-only state before every write — browser panes, detected ports, manual
  monitor flags, agent browser sessions, the snapshot revision, and every pane status
  except `Review`.
- **`observability.json`** (`~/.local/share/<APP_DIR_NAME>/observability.json`) — feature
  flags, metrics, and a 300-entry log ring, re-serialized on every log line and metric
  mutation. It is deliberately per-build so a flag toggled in a dev build cannot leak into
  the installed one.
- **`agent_chat_promoted`** (a sentinel *file* beside it) — the one-time Agent Chat
  promotion marker. It is a standalone file rather than only a JSON key precisely because
  an older binary rewrites `observability.json` without fields it doesn't know, which would
  erase a snapshot-only marker and force-revert a deliberate opt-out. Old binaries never
  touch the file.
- **`settings-cache.json` + `settings-dirty`** — the synced user-settings blob (theme,
  fonts, keybinds, notifications). Not the `settings` table.
- Per-repo files under `<project>/.codemux/`: `config.json`, `ports.json`,
  `project-memory.json`, `index.json`.

The JSON convention is **serde-additive**: optional fields carry `#[serde(default)]`, no
struct uses `deny_unknown_fields`, and the whole snapshot is written and emitted wholesale
so a new field costs no plumbing. `layout.json` carries its own `schema_version`, which —
like the SQLite one — is written but never checked; layout evolution runs instead as ad-hoc
boot passes (`migrate_tabs_if_needed`, `migrate_project_roots`).

### Concurrency

One long-lived connection behind a `std::sync::Mutex` per process
(`DatabaseStore { conn: Mutex<Connection> }`), opened once, with
`journal_mode=WAL` and `foreign_keys=ON`. `AppStateStore` is a *separate* mutex over the
in-memory snapshot with no reference to the database and no shared lock, so the two stores
have no transactional relationship.

## What Works Today

- A fresh install and an install upgrading from any older version converge on the same
  schema through one replayed, idempotent code path.
- Adding a column is a two-line change that cannot break existing users if the convention
  is followed, and the ledger doubles as a readable history of the schema.
- Foreign keys are enforced, and chat messages and checkpoints cascade with their thread.
- WAL lets the CLI read while the desktop app is running.
- Secrets are not stored in the clear: account sessions are AES-256-GCM encrypted, and
  web-remote tokens are stored only as SHA-256 hashes.
- Dev and release builds keep fully separate app data (the remote daemon excepted).
- Cloud-synced tables carry `server_id` / `dirty` / `deleted_at` so sync is incremental and
  deletes are tombstoned until the server acknowledges them.

## Current Constraints

- **No rollback and no version gate.** `SCHEMA_VERSION` is write-only. A newer-schema
  database opened by an older binary has its version silently rewritten downward and is
  then used as-is; the extra columns physically survive but the old binary's explicit
  column lists leave them at `NULL`. Nothing detects or warns.
- **The idempotency check is a substring match** on the text `duplicate column name`. A
  change in SQLite/rusqlite error wording would silently convert idempotency into a hard
  startup failure — which degrades to the in-memory fallback.
- **No `busy_timeout`, no retry, and no explicit transactions.** Up to three processes open
  the desktop database concurrently; a writer collision surfaces as an immediate
  `SQLITE_BUSY`. Multi-statement flows are individually autocommitted, so a crash mid-flow
  can leave them half-applied — as can a crash between a `layout.json` write and a database
  write, since the two stores are independent.
- **Failures are quiet.** Schema failure degrades to an in-memory database with only a
  stderr warning; a corrupt file is an accepted data-loss outcome (there is a test that
  documents this); a `layout.json` parse error silently resets the entire layout; several
  write paths discard their result.
- **A poisoned mutex is fatal for the process.** Every lock is `.unwrap()`ed, so a panic
  while holding the database lock makes every later database call panic.
- **The retired-workspace-kind sink is a downgrade data-loss path.** `WorkspaceType` has a
  `#[serde(other)] Removed` catch-all, and stripped workspaces (with their terminal
  sessions) are deleted at boot *and* before every persist. That is correct for genuinely
  retired kinds, but it means a workspace kind written by a **newer** binary deserializes as
  `Removed` in an older one and is permanently removed on its next write. Retiring a variant
  therefore requires adding a `#[serde(alias = ...)]` on its replacement **in the same
  commit**, or every legacy workspace of that kind is wiped.
- **Nothing prunes or vacuums.** `agent_chat_messages` is the dominant growth vector — every
  provider event is a retained JSON row, uncapped, until its thread is deleted.
  `automation_runs` and revoked `web_remote_sessions` are kept indefinitely by design, and
  `settings`/`ui_state`/`recent_projects` accumulate one entry per project ever touched. A
  user who never signs in accumulates `dirty` sync tombstones forever, because they are
  hard-deleted only on server acknowledgement. There is no `VACUUM` in the repo.
- `layout.json` is rewritten in full every 500 ms of activity, and `observability.json` in
  full on every log line.

## How To Change The Schema

**Add a table:** add the `CREATE TABLE IF NOT EXISTS` (plus indexes) to the batch in
`create_schema`, and bump `SCHEMA_VERSION`. Existing installs pick it up on next launch.

**Add a column — both edits are mandatory:**

1. Add it to the `CREATE TABLE IF NOT EXISTS` body, so fresh installs get it.
2. **Also** append `ALTER TABLE <t> ADD COLUMN <c> <type>` to the migration array, so
   existing installs get it. Skipping step 2 is invisible on a dev machine whose database
   was recreated and breaks every upgrading user — this is the exact failure the convention
   exists to prevent.
3. A `NOT NULL` column must carry a `DEFAULT`; choose one that reads back correctly for
   pre-existing rows.
4. Make the Rust field tolerate legacy rows: `Option<T>` plus `#[serde(default)]`.
5. If old rows need healing, append an idempotent `UPDATE ... WHERE <col> IS NULL` **after**
   its `ALTER` — ordering is load-bearing.
6. Bump `SCHEMA_VERSION`.

**Never** remove an entry from the migration array — it is an append-only ledger replayed by
every upgrading user — and never add a `DROP COLUMN` or other reversing statement, which
would hard-error on the second run instead of producing a swallowed duplicate-column error.
To drop a retired table, use `DROP TABLE IF EXISTS` in the array and delete its
`CREATE TABLE` from the batch; `openflow_history` is the worked example.

**Add a persisted layout field:** add it to the snapshot struct with `#[serde(default)]`,
and decide explicitly whether it is runtime-only — if so, strip it in
`persistable_snapshot`. No migration is needed; old files deserialize with the default.

The remote daemon follows the same rules in `remote/workspace.rs`, except there is no
version constant and indexes for newly added columns go in the trailing batch.

## Important Touch Points

- `src-tauri/src/database.rs` — the desktop store, schema, and migration ledger
- `src-tauri/src/commands/database.rs` — the Tauri command surface over it
- `src-tauri/src/state/state_impl.rs` — `AppStateSnapshot`, `layout.json` load/save, the
  debouncer, `persistable_snapshot`, and the retired-kind sink
- `src-tauri/src/observability.rs` — `observability.json`, feature flags, the promotion sentinel
- `src-tauri/src/remote/workspace.rs`, `src-tauri/src/remote/config.rs` — the daemon's separate store
- `src-tauri/src/lib.rs` — `APP_DIR_NAME`, database registration, the in-memory fallback
- `docs/features/session-persistence.md` — scrollback and agent session resume
- `docs/features/settings-sync.md`, `docs/features/workspaces-sync.md` — what leaves the device

## Notes

- Keep this file about current truth, not future plans.
- If a future change makes `SCHEMA_VERSION` actually gate behavior, this doc's central
  claim changes — update it in the same commit.
