use rusqlite::{params, params_from_iter, types::Value, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;

const SCHEMA_VERSION: u32 = 16;

pub struct DatabaseStore {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentProject {
    pub path: String,
    pub name: String,
    pub last_opened_at: String,
}

/// Persisted record for an agent-chat session so the history dropdown
/// can reopen prior chats after the app restarts. One row per pane's
/// lifetime thread_id; the `sdk_session_id` column carries the Claude
/// Agent SDK session UUID (populated once the SDK's first message
/// lands) and is what the "resume" path feeds back into
/// `StartSessionInput::resume_cursor`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChatSessionRecord {
    pub thread_id: String,
    pub sdk_session_id: Option<String>,
    pub workspace_id: String,
    pub cwd: Option<String>,
    pub provider: String,
    pub title: Option<String>,
    pub created_at: String,
    pub last_active_at: String,
    /// Per-thread model id the pane was last configured with. Persisted
    /// so a pane reopened after an app restart re-seeds the user's chosen
    /// model instead of falling back to the provider default, and so the
    /// backend auto-resume path (`ensure_live_session`) restarts the SDK
    /// session with the same model. `None` for legacy rows created before
    /// this column existed.
    #[serde(default)]
    pub model: Option<String>,
    /// Per-thread reasoning / effort level (Claude: session-scoped).
    #[serde(default)]
    pub effort: Option<String>,
    /// Per-thread context-window selection (Claude `"1m"` etc.).
    #[serde(default)]
    pub context_window: Option<String>,
    /// Per-thread permission mode (`default` / `acceptEdits` / …).
    #[serde(default)]
    pub permission_mode: Option<String>,
    /// Per-thread premium speed-tier choice. A missing database value maps to
    /// `false` for rows created before this column existed.
    #[serde(default)]
    pub fast_mode: bool,
}

/// Lightweight provider-neutral conversation row used by the composer's
/// `@session:` picker. Unlike the history dropdown this is not a resume
/// surface: sessions only need human-visible transcript prose, not a live
/// provider cursor. That lets completed conversations remain attachable even
/// if their provider-side resume record has disappeared.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentChatSessionMention {
    pub thread_id: String,
    pub workspace_id: String,
    pub cwd: Option<String>,
    pub provider: String,
    pub title: Option<String>,
    pub last_active_at: String,
    pub preview: String,
    pub message_count: u32,
}

/// One safe transcript entry for a session handoff. The backing FTS table
/// deliberately contains only user messages and top-level assistant prose;
/// tool output, hidden reasoning, requests, and subagent internals never enter
/// this shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentChatVisibleMessage {
    pub role: String,
    pub content: String,
}

/// One cursor-addressable safe conversation message returned by the Codemux
/// history tools. This is deliberately sourced from `agent_chat_search`, so
/// hidden reasoning, tool payloads, requests, and subagent internals cannot
/// leak through the on-demand handoff path either.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentChatHistoryMessage {
    pub message_id: i64,
    pub role: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentChatHistoryPage {
    pub conversation_id: String,
    pub title: Option<String>,
    pub provider: String,
    pub messages: Vec<AgentChatHistoryMessage>,
    pub next_cursor: Option<i64>,
    pub total_visible_messages: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentChatHistorySearchHit {
    pub message_id: i64,
    pub role: String,
    pub snippet: String,
    pub created_at: String,
}

/// Versioned cache for the expensive model-written portion of a conversation
/// handoff. Direct transcript fallback is deterministic and is never cached.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentChatHandoffSummaryCache {
    pub revision_message_id: i64,
    pub summarizer_provider: String,
    pub summarizer_model: String,
    pub summarizer_effort: Option<String>,
    pub prompt_version: u32,
    pub summary: String,
    pub generated_at: String,
}

/// One hit in the durable conversation index. `message_id` points back to
/// the persisted event so the frontend can reopen the thread and jump to the
/// matching turn; title-only hits intentionally leave it empty.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentChatSearchResult {
    pub message_id: Option<i64>,
    pub thread_id: String,
    pub workspace_id: String,
    pub cwd: Option<String>,
    pub provider: String,
    pub session_title: Option<String>,
    /// `user`, `assistant`, or `title`.
    pub role: String,
    pub turn_id: Option<String>,
    pub snippet: String,
    pub created_at: String,
}

/// Per-thread chat configuration written alongside an
/// `agent_chat_sessions` row. Every field is a tri-state so callers
/// (session start, the picker-driven update command, auto-resume) can
/// express three distinct intents per column:
///
/// - `None` (outer) — field absent: leave the stored value untouched.
/// - `Some(None)` — explicit clear: set the column to `NULL`.
/// - `Some(Some(v))` — set the column to `v`.
///
/// The tri-state matters for the model-change compat reset: switching
/// to a model that has no effort levels / no 1m context must be able to
/// CLEAR `effort` / `context_window` back to `NULL`, otherwise the stale
/// value is resurrected on the next restart (it would be paired with a
/// model that doesn't support it). A plain `Option<String>` can't
/// express "clear" — a JSON `null` and an absent field both deserialize
/// to `None` — so the fields are `Option<Option<String>>` with a
/// custom deserializer that maps an explicit `null` to `Some(None)`.
/// See [`DatabaseStore::update_agent_chat_session_config`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentChatSessionConfig {
    #[serde(default, deserialize_with = "deserialize_tristate")]
    pub model: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_tristate")]
    pub effort: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_tristate")]
    pub context_window: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_tristate")]
    pub permission_mode: Option<Option<String>>,
    /// Unlike the nullable string fields, fast mode has a concrete standard
    /// state, so a plain optional boolean can express leave/set-false/set-true.
    #[serde(default)]
    pub fast_mode: Option<bool>,
}

/// Deserialize a present field (whether `null` or a value) into the
/// inner `Some(..)` of an [`AgentChatSessionConfig`] tri-state, so an
/// explicit JSON `null` becomes `Some(None)` (clear) rather than being
/// indistinguishable from an absent field. Absent fields fall through
/// to `#[serde(default)]` = `None` (leave untouched).
fn deserialize_tristate<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<String>::deserialize(deserializer)?))
}

impl AgentChatSessionConfig {
    /// Build a config that overwrites a column with a concrete value.
    /// Convenience for callers that always set (never clear) a field.
    pub fn set(value: impl Into<String>) -> Option<Option<String>> {
        Some(Some(value.into()))
    }

    /// Map a plain `Option<String>` (the shape most internal callers
    /// hold) into the tri-state with "leave untouched on `None`"
    /// semantics: `Some(v)` → set to `v`, `None` → leave the column
    /// unchanged. Use for session-start / auto-resume snapshots that
    /// should never clear a column they simply don't know about.
    pub fn keep_or_set(value: Option<String>) -> Option<Option<String>> {
        value.map(Some)
    }
}

/// Persisted bookkeeping for the run-start rollback checkpoint
/// (issue #80): the background working-tree snapshot taken when an
/// agent-chat session starts. The snapshot itself lives in the repo's
/// object database, anchored on `ref_name`; this row records where it
/// is and what state to restore (`head_commit` / `branch`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChatCheckpointRecord {
    pub thread_id: String,
    pub workspace_id: String,
    pub repo_path: String,
    pub ref_name: String,
    pub snapshot_commit: String,
    pub head_commit: String,
    pub branch: Option<String>,
    pub created_at: String,
}

/// One exact pre-dispatch workspace snapshot in a thread's contiguous
/// provider conversation timeline.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentChatTurnCheckpointRecord {
    pub thread_id: String,
    pub workspace_id: String,
    pub repo_path: String,
    pub turn_index: i64,
    pub client_nonce: Option<String>,
    pub transcript_cutoff_id: i64,
    pub ref_name: String,
    pub snapshot_commit: String,
    pub head_commit: String,
    pub branch: Option<String>,
    pub created_at: String,
}

/// Filesystem state a committed true revert left behind. Git refs and chat
/// image files live outside SQLite, so the command layer reclaims them after
/// the revert transaction commits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentChatTurnRevertOutcome {
    /// `(repo_path, ref_name)` checkpoint refs the revert made obsolete.
    pub removed_refs: Vec<(String, String)>,
    /// Payloads of the trimmed transcript rows that carried attachments.
    pub removed_image_payloads: Vec<String>,
    /// Payloads of the RETAINED rows that carried attachments — a file a
    /// surviving turn still references must not be deleted (a re-sent
    /// attachment can appear on both sides of the cutoff).
    pub retained_image_payloads: Vec<String>,
}

/// Filesystem state whose owning session rows were removed while duplicate
/// provider sessions were collapsed. The command layer uses this after the
/// database transaction commits to remove external image directories and Git
/// refs that SQLite cannot cascade-delete itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentChatSessionCleanup {
    pub thread_ids: Vec<String>,
    /// `(thread_id, repo_path)` pairs that may own a pre-restore safety ref.
    pub repo_paths: Vec<(String, String)>,
    /// Exact `(repo_path, ref_name)` checkpoint refs to delete.
    pub checkpoint_refs: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectScripts {
    #[serde(default)]
    pub setup: Vec<String>,
    #[serde(default)]
    pub teardown: Vec<String>,
    #[serde(default)]
    pub run: Option<String>,
    #[serde(default)]
    pub worktree_includes: Vec<String>,
}

/// A paired web-remote browser device. One row in `web_remote_sessions`.
/// `token_hash` is the SHA-256 (hex) of the bearer token handed to the
/// browser at pair time; the plaintext token is never persisted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebRemoteSessionRecord {
    pub id: String,
    pub name: Option<String>,
    pub user_agent: Option<String>,
    #[serde(skip_serializing)]
    pub token_hash: String,
    pub created_at: String,
    pub last_seen_at: Option<String>,
    pub approved: bool,
    pub revoked: bool,
    /// How this session was admitted: `"pair"` (pairing-token QR/link) or
    /// `"account"` (proved ownership of the desktop's signed-in Codemux
    /// account via `POST /api/pair-account`). Legacy rows written before this
    /// column existed default to `"pair"`.
    pub source: String,
    /// For `source = "account"` sessions, the Codemux account `user.id` that
    /// was verified to equal the desktop's own signed-in user at admission
    /// time. `None` for pairing-token sessions.
    pub account_user_id: Option<String>,
}

fn database_path() -> Option<PathBuf> {
    let config = dirs::config_dir()?;
    Some(config.join(crate::APP_DIR_NAME).join("codemux.db"))
}

fn create_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS settings (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id TEXT NOT NULL DEFAULT 'local',
            key TEXT NOT NULL,
            value TEXT NOT NULL,
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(user_id, key)
        );

        CREATE TABLE IF NOT EXISTS projects (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id TEXT NOT NULL DEFAULT 'local',
            name TEXT NOT NULL,
            path TEXT NOT NULL,
            color TEXT,
            tab_order INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(user_id, path)
        );

        CREATE TABLE IF NOT EXISTS workspace_state (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id TEXT NOT NULL DEFAULT 'local',
            workspace_id TEXT NOT NULL,
            project_id INTEGER REFERENCES projects(id),
            tab_order INTEGER NOT NULL DEFAULT 0,
            is_collapsed INTEGER NOT NULL DEFAULT 0,
            last_active_at TEXT,
            UNIQUE(user_id, workspace_id)
        );

        CREATE TABLE IF NOT EXISTS ui_state (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id TEXT NOT NULL DEFAULT 'local',
            key TEXT NOT NULL,
            value TEXT NOT NULL,
            UNIQUE(user_id, key)
        );

        CREATE TABLE IF NOT EXISTS recent_projects (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id TEXT NOT NULL DEFAULT 'local',
            path TEXT NOT NULL,
            name TEXT NOT NULL,
            last_opened_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(user_id, path)
        );

        CREATE TABLE IF NOT EXISTS auth_tokens (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            encrypted_data BLOB NOT NULL,
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS agent_chat_sessions (
            thread_id TEXT PRIMARY KEY,
            sdk_session_id TEXT,
            workspace_id TEXT NOT NULL,
            cwd TEXT,
            provider TEXT NOT NULL,
            title TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_active_at TEXT NOT NULL DEFAULT (datetime('now')),
            -- Per-thread chat configuration so a pane reopened after an
            -- app restart re-seeds the user's chosen model / effort /
            -- context-window / permission-mode instead of the provider
            -- default, and so backend auto-resume restarts the SDK
            -- session with the same settings. Nullable for legacy rows.
            model TEXT,
            effort TEXT,
            context_window TEXT,
            permission_mode TEXT,
            fast_mode INTEGER
        );

        CREATE INDEX IF NOT EXISTS idx_agent_chat_sessions_workspace
            ON agent_chat_sessions(workspace_id, last_active_at DESC);
        CREATE INDEX IF NOT EXISTS idx_agent_chat_sessions_cwd
            ON agent_chat_sessions(cwd, last_active_at DESC);

        CREATE TABLE IF NOT EXISTS agent_chat_messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            thread_id TEXT NOT NULL,
            payload TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (thread_id)
                REFERENCES agent_chat_sessions(thread_id)
                ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_agent_chat_messages_thread
            ON agent_chat_messages(thread_id, id ASC);

        -- Lazy, model-written conversation handoffs (schema v16). One cache
        -- row per source conversation is sufficient: changing utility model,
        -- prompt version, or source revision replaces it atomically. The
        -- source transcript remains authoritative and cascade deletion keeps
        -- this derived data from outliving it.
        CREATE TABLE IF NOT EXISTS agent_chat_handoff_summaries (
            thread_id TEXT PRIMARY KEY,
            revision_message_id INTEGER NOT NULL,
            summarizer_provider TEXT NOT NULL,
            summarizer_model TEXT NOT NULL,
            summarizer_effort TEXT,
            prompt_version INTEGER NOT NULL,
            summary TEXT NOT NULL,
            generated_at TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (thread_id)
                REFERENCES agent_chat_sessions(thread_id)
                ON DELETE CASCADE
        );

        -- Full-text conversation index (schema v14). Its rowid is the
        -- durable source message id. Keep only human-visible conversation
        -- prose: user messages and top-level assistant text. Tool payloads,
        -- reasoning, workflow state, and subagent internals stay out.
        CREATE VIRTUAL TABLE IF NOT EXISTS agent_chat_search USING fts5(
            content,
            thread_id UNINDEXED,
            role UNINDEXED,
            turn_id UNINDEXED,
            tokenize = 'unicode61 remove_diacritics 2'
        );

        CREATE TRIGGER IF NOT EXISTS agent_chat_search_after_insert
        AFTER INSERT ON agent_chat_messages
        BEGIN
            INSERT INTO agent_chat_search(rowid, content, thread_id, role, turn_id)
            SELECT
                NEW.id,
                CASE
                    WHEN json_extract(NEW.payload, '$.type') = 'user_message'
                        THEN json_extract(NEW.payload, '$.text')
                    ELSE json_extract(NEW.payload, '$.item.text')
                END,
                NEW.thread_id,
                CASE
                    WHEN json_extract(NEW.payload, '$.type') = 'user_message'
                        THEN 'user'
                    ELSE 'assistant'
                END,
                CASE
                    WHEN json_extract(NEW.payload, '$.type') = 'item_completed'
                        THEN json_extract(NEW.payload, '$.turn_id')
                    ELSE NULL
                END
            WHERE json_valid(NEW.payload)
              AND (
                    (
                        json_extract(NEW.payload, '$.type') = 'user_message'
                        AND length(trim(COALESCE(json_extract(NEW.payload, '$.text'), ''))) > 0
                    )
                    OR
                    (
                        json_extract(NEW.payload, '$.type') = 'item_completed'
                        AND json_extract(NEW.payload, '$.item.kind') = 'assistant_text'
                        AND json_extract(NEW.payload, '$.subagent_id') IS NULL
                        AND length(trim(COALESCE(json_extract(NEW.payload, '$.item.text'), ''))) > 0
                    )
              );
        END;

        CREATE TRIGGER IF NOT EXISTS agent_chat_search_after_delete
        AFTER DELETE ON agent_chat_messages
        BEGIN
            DELETE FROM agent_chat_search WHERE rowid = OLD.id;
        END;

        -- Thread ids change when a provider promotes or de-duplicates a
        -- persisted session, so updates have to move their FTS rows too.
        CREATE TRIGGER IF NOT EXISTS agent_chat_search_after_update
        AFTER UPDATE OF thread_id, payload ON agent_chat_messages
        BEGIN
            DELETE FROM agent_chat_search WHERE rowid = OLD.id;
            INSERT INTO agent_chat_search(rowid, content, thread_id, role, turn_id)
            SELECT
                NEW.id,
                CASE
                    WHEN json_extract(NEW.payload, '$.type') = 'user_message'
                        THEN json_extract(NEW.payload, '$.text')
                    ELSE json_extract(NEW.payload, '$.item.text')
                END,
                NEW.thread_id,
                CASE
                    WHEN json_extract(NEW.payload, '$.type') = 'user_message'
                        THEN 'user'
                    ELSE 'assistant'
                END,
                CASE
                    WHEN json_extract(NEW.payload, '$.type') = 'item_completed'
                        THEN json_extract(NEW.payload, '$.turn_id')
                    ELSE NULL
                END
            WHERE json_valid(NEW.payload)
              AND (
                    (
                        json_extract(NEW.payload, '$.type') = 'user_message'
                        AND length(trim(COALESCE(json_extract(NEW.payload, '$.text'), ''))) > 0
                    )
                    OR
                    (
                        json_extract(NEW.payload, '$.type') = 'item_completed'
                        AND json_extract(NEW.payload, '$.item.kind') = 'assistant_text'
                        AND json_extract(NEW.payload, '$.subagent_id') IS NULL
                        AND length(trim(COALESCE(json_extract(NEW.payload, '$.item.text'), ''))) > 0
                    )
              );
        END;

        -- Usage ledger behind Settings → Usage. Most rows are a
        -- materialized local cache of provider-owned history (Claude/Codex
        -- transcripts and OpenCode storage). Grok is the exception: its ACP
        -- PromptResponse is the durable provider-owned bill, so exact
        -- per-turn rows are recorded live with a stable import key.
        --
        -- Deliberately carries NO foreign key to `agent_chat_sessions`.
        -- Every other chat-adjacent table cascades on session delete,
        -- which is right for transcript state and wrong for accounting:
        -- deleting a chat from the history dropdown must not silently
        -- rewrite last month's spend. `thread_id` and `workspace_id` are
        -- therefore denormalized copies, valid after their sources are
        -- gone.
        --
        -- The four token columns are non-overlapping, so a token total is
        -- their plain sum. `cost_usd` is the provider-reported or static
        -- API-equivalent estimate frozen when the cache row is derived.
        CREATE TABLE IF NOT EXISTS agent_usage_ledger (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            created_at INTEGER NOT NULL,     -- unix ms
            thread_id TEXT NOT NULL,
            workspace_id TEXT,
            provider TEXT NOT NULL,
            model TEXT,
            subagent INTEGER NOT NULL DEFAULT 0,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cache_read_tokens INTEGER NOT NULL DEFAULT 0,
            cache_write_tokens INTEGER NOT NULL DEFAULT 0,
            -- Informational SUBSET of output_tokens, never added into a
            -- total. Present so the dashboard can report how much of the
            -- output was reasoning; 0 when the provider reports no split.
            reasoning_tokens INTEGER NOT NULL DEFAULT 0,
            cost_usd REAL,
            -- provider (upstream catalogue rates) | table (our static
            -- price list) | NULL (unpriced). Lets the UI show how much of
            -- a total is measured rather than estimated.
            cost_source TEXT,
            -- `provider_history`: derived from durable local history and safe
            -- to rebuild; `live`: an exact provider bill with no separate
            -- history importer (currently Grok).
            source TEXT NOT NULL DEFAULT 'provider_history',
            -- Provider-native idempotency key. Every history row has one.
            import_key TEXT
        );


        -- Signatures of provider history sources, so unchanged files and
        -- databases can be skipped. Keyed by absolute path.
        CREATE TABLE IF NOT EXISTS usage_import_state (
            path TEXT PRIMARY KEY,
            mtime_ms INTEGER NOT NULL,
            size_bytes INTEGER NOT NULL,
            consumed_bytes INTEGER NOT NULL,
            scanned_at INTEGER NOT NULL
        );

        -- Every dashboard query is a time-range scan.
        CREATE INDEX IF NOT EXISTS idx_agent_usage_ledger_created
            ON agent_usage_ledger(created_at);

        -- Run-start rollback checkpoints (issue #80). One row per
        -- thread: the background snapshot taken when the agent-chat
        -- session started. `ref_name` is the shadow git ref anchoring
        -- the snapshot (refs/codemux/checkpoints/<id>); pruning that
        -- ref deletes this row too. Cascade with the session so
        -- deleting a chat from the history dropdown cleans up its
        -- checkpoint bookkeeping.
        CREATE TABLE IF NOT EXISTS agent_chat_checkpoints (
            thread_id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL,
            repo_path TEXT NOT NULL,
            ref_name TEXT NOT NULL,
            snapshot_commit TEXT NOT NULL,
            head_commit TEXT NOT NULL,
            branch TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            FOREIGN KEY (thread_id)
                REFERENCES agent_chat_sessions(thread_id)
                ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_agent_chat_checkpoints_ref
            ON agent_chat_checkpoints(ref_name);

        -- Turn-addressable checkpoints. These are deliberately separate from
        -- the legacy run-start row: a target is valid only while this ordered
        -- timeline remains contiguous with the provider's native history.
        CREATE TABLE IF NOT EXISTS agent_chat_turn_checkpoints (
            thread_id TEXT NOT NULL,
            turn_index INTEGER NOT NULL,
            workspace_id TEXT NOT NULL,
            repo_path TEXT NOT NULL,
            client_nonce TEXT,
            transcript_cutoff_id INTEGER NOT NULL,
            ref_name TEXT NOT NULL,
            snapshot_commit TEXT NOT NULL,
            head_commit TEXT NOT NULL,
            branch TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (thread_id, turn_index),
            FOREIGN KEY (thread_id)
                REFERENCES agent_chat_sessions(thread_id)
                ON DELETE CASCADE
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_chat_turn_checkpoints_ref
            ON agent_chat_turn_checkpoints(repo_path, ref_name);
        CREATE INDEX IF NOT EXISTS idx_agent_chat_turn_checkpoints_nonce
            ON agent_chat_turn_checkpoints(thread_id, client_nonce);

        -- Hosts (Step 2 of cloud push — Settings → Hosts pane data model).
        --
        -- Each row is a user-defined SSH target plus a friendly name. The
        -- workspace will eventually carry a `host_id` pointing at one of
        -- these (or NULL meaning local). SSH credentials are NOT stored
        -- here and never leave the device — they live in ~/.ssh/. This
        -- table holds only the *identity* of the remote box.
        --
        -- `server_id` is the row id assigned by the API when this host
        -- syncs to the cloud, used to correlate local <-> server rows on
        -- merge. NULL until the first successful push.
        --
        -- `deleted_at` is a soft-delete tombstone so deletions sync
        -- cleanly: we keep the row locally with a deletion timestamp,
        -- push the delete, then the next pull will see it gone from
        -- the server and we can hard-delete locally. Matches the
        -- pattern Vexis uses for voice data lifecycle.
        --
        -- `dirty` flag mirrors the settings-sync model: 1 means the
        -- local row has unpushed changes, 0 means it matches the
        -- last-known server state. Lets `hosts_sync` push only what
        -- changed.
        CREATE TABLE IF NOT EXISTS hosts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id TEXT NOT NULL DEFAULT 'local',
            server_id TEXT,
            name TEXT NOT NULL,
            ssh_target TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            deleted_at TEXT,
            dirty INTEGER NOT NULL DEFAULT 1,
            -- Local-only observation columns written by the inventory
            -- poller (`hosts_status`): when THIS install last reached the
            -- host over SSH and the last workspace-disk measurement with
            -- its timestamp. They are never pushed, and a server pull
            -- (`upsert_host_from_server`) must never overwrite them —
            -- another device's view of 'last seen' is meaningless here.
            last_seen_at TEXT,
            disk_bytes INTEGER,
            disk_measured_at TEXT,
            UNIQUE(user_id, server_id)
        );

        CREATE INDEX IF NOT EXISTS idx_hosts_user
            ON hosts(user_id, deleted_at);
        CREATE INDEX IF NOT EXISTS idx_hosts_dirty
            ON hosts(user_id, dirty)
            WHERE dirty = 1;

        -- Automations — scheduled agent runs on a chosen host.
        --
        -- An automation is a named prompt + agent + recurrence. The
        -- recurrence lives in `schedule` as a complete iCalendar block
        -- (a DTSTART line plus one RRULE line, RFC 5545); `timezone`
        -- holds the IANA zone for display. The host-side scheduler
        -- (codemux-remote) reads `next_run_at` to decide when to fire.
        --
        -- `host_id` is a plain integer, NOT a foreign key: hosts are
        -- soft-deleted then physically purged by the sync layer, and a
        -- hard FK would block that purge. A dangling `host_id` is
        -- surfaced to the user as a removed-host state rather than
        -- corrupting the row.
        --
        -- `server_id` / `deleted_at` / `dirty` mirror the `hosts` table
        -- so the account-sync layer (a future `automations_sync`) can
        -- be added with no schema migration.
        CREATE TABLE IF NOT EXISTS automations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id TEXT NOT NULL DEFAULT 'local',
            server_id TEXT,
            name TEXT NOT NULL,
            prompt TEXT NOT NULL,
            agent TEXT NOT NULL DEFAULT 'claude',
            schedule TEXT NOT NULL,
            timezone TEXT NOT NULL DEFAULT 'UTC',
            host_id INTEGER,
            project_path TEXT,
            project_remote TEXT,
            enabled INTEGER NOT NULL DEFAULT 1,
            retention_limit INTEGER NOT NULL DEFAULT 10,
            last_run_at TEXT,
            next_run_at TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            deleted_at TEXT,
            dirty INTEGER NOT NULL DEFAULT 1,
            UNIQUE(user_id, server_id)
        );

        CREATE INDEX IF NOT EXISTS idx_automations_user
            ON automations(user_id, deleted_at);
        CREATE INDEX IF NOT EXISTS idx_automations_dirty
            ON automations(user_id, dirty)
            WHERE dirty = 1;

        -- Automation runs — one row per fire (or skipped fire).
        --
        -- `UNIQUE(automation_id, scheduled_for)` is the idempotency key:
        -- `scheduled_for` is floored to the minute by the caller, so a
        -- re-delivered tick for the same minute is a no-op insert. Run
        -- rows are kept indefinitely (history is cheap, text-only); only
        -- the agent worktrees on the host are pruned, per the
        -- automation's `retention_limit`.
        CREATE TABLE IF NOT EXISTS automation_runs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            automation_id INTEGER NOT NULL
                REFERENCES automations(id) ON DELETE CASCADE,
            status TEXT NOT NULL DEFAULT 'scheduled',
            scheduled_for TEXT NOT NULL,
            started_at TEXT,
            finished_at TEXT,
            host_id INTEGER,
            workspace_id TEXT,
            branch TEXT,
            pr_url TEXT,
            error TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(automation_id, scheduled_for)
        );

        CREATE INDEX IF NOT EXISTS idx_automation_runs_automation
            ON automation_runs(automation_id, scheduled_for DESC);

        -- Workspaces sync mirror (cross-device workspace registry).
        --
        -- Holds the metadata that lets *other devices* of the same
        -- account discover that a workspace exists and (later) pull
        -- it back. Runtime state — pane tree, terminal sessions, git
        -- stats, notification count — lives in the app_state JSON
        -- blob and is per-device. This table is JUST the synced
        -- identity + host + project metadata.
        --
        -- `workspace_id` is THIS device local id (e.g. workspace-42
        -- from next_id), or NULL when the row was pulled from
        -- another device and hasn't been adopted here yet. The
        -- originating device local id is never re-used cross-device
        -- — every device assigns its own when adopting.
        --
        -- `server_id` is the cross-device identity (the
        -- `codemux_workspaces.id` BIGSERIAL stringified). NULL until
        -- this row's first successful push.
        --
        -- `host_server_id` is the cross-device host identity
        -- (`codemux_hosts.id` stringified). NULL means the workspace
        -- lives on the device that created the row. Maps back to a
        -- local `hosts.id` via the `hosts.server_id` column.
        --
        -- Same soft-delete + dirty model as the `hosts` and
        -- `automations` tables. `workspaces_sync` is intentionally a
        -- separate table from `workspace_state` because the latter
        -- is per-device runtime UI state (tab order, collapse), not
        -- cross-device identity.
        CREATE TABLE IF NOT EXISTS workspaces_sync (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id TEXT NOT NULL DEFAULT 'local',
            server_id TEXT,
            workspace_id TEXT,
            title TEXT NOT NULL,
            host_server_id TEXT,
            project_path TEXT,
            project_remote TEXT,
            git_branch TEXT,
            default_branch TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            deleted_at TEXT,
            dirty INTEGER NOT NULL DEFAULT 1,
            UNIQUE(user_id, server_id),
            UNIQUE(user_id, workspace_id)
        );

        CREATE INDEX IF NOT EXISTS idx_workspaces_sync_user
            ON workspaces_sync(user_id, deleted_at);
        CREATE INDEX IF NOT EXISTS idx_workspaces_sync_dirty
            ON workspaces_sync(user_id, dirty)
            WHERE dirty = 1;
        CREATE INDEX IF NOT EXISTS idx_workspaces_sync_host
            ON workspaces_sync(user_id, host_server_id);

        -- Web remote access: one row per paired browser device (schema v8).
        -- The session token is never stored in the clear — `token_hash` holds
        -- its SHA-256 and the auth layer does a constant-time compare. A
        -- device stays in this table (revoked = 1) after revocation so the
        -- desktop UI can still show a history entry; the auth layer treats any
        -- revoked = 1 row as dead. `approved` gates ws-ticket issuance when the
        -- server runs in require-approval mode. See web_remote::auth.
        CREATE TABLE IF NOT EXISTS web_remote_sessions (
            id TEXT PRIMARY KEY,
            name TEXT,
            user_agent TEXT,
            token_hash TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_seen_at TEXT,
            approved INTEGER NOT NULL DEFAULT 0,
            revoked INTEGER NOT NULL DEFAULT 0
        );
        ",
    )
    .map_err(|e| format!("Failed to create database schema: {e}"))?;

    // Additive column migrations. `CREATE TABLE IF NOT EXISTS` above is a
    // no-op on a database that already has the table, so a column added
    // to an existing table needs an explicit `ALTER TABLE`. Each is
    // idempotent: on a fresh database the column is already present and
    // SQLite returns a "duplicate column name" error, which we swallow.
    for stmt in [
        // The retired orchestration feature owned this table. Removing it here
        // reclaims existing installs' stale run history; fresh databases never
        // create it.
        "DROP TABLE IF EXISTS openflow_history",
        "ALTER TABLE automations ADD COLUMN project_remote TEXT",
        // Devices page: local-only host observations (see the `hosts`
        // CREATE above for why a server pull must never touch these).
        "ALTER TABLE hosts ADD COLUMN last_seen_at TEXT",
        "ALTER TABLE hosts ADD COLUMN disk_bytes INTEGER",
        "ALTER TABLE hosts ADD COLUMN disk_measured_at TEXT",
        "ALTER TABLE automation_runs ADD COLUMN branch TEXT",
        "ALTER TABLE automation_runs ADD COLUMN pr_url TEXT",
        // Phase-4 divergence detection: workspace git HEAD sha so
        // the overview can flag when the same project+branch
        // exists on multiple devices with different HEADs.
        "ALTER TABLE workspaces_sync ADD COLUMN git_head_sha TEXT",
        // Auto-publish: stable UUID assigned by the remote daemon
        // (`remote::workspace::Workspace.id`) when the desktop
        // discovers a workspace by polling a host. Lets the
        // host-inventory reconcile pass dedupe across repeated polls
        // — i.e. "did I already insert a sync row for this host's
        // workspace UUID?" Always null for rows that originated on
        // this device or arrived purely via cloud pull.
        "ALTER TABLE workspaces_sync ADD COLUMN origin_uid TEXT",
        // First-class project identity (see `project_identity.rs`).
        // Deterministic `UUIDv5(canonical remote ?? project_root)` so a
        // project's main checkout + worktrees group together and the
        // same repo converges across hosts/devices. `workspace_kind` is
        // 'main' | 'worktree'. Set locally by the host-inventory poller
        // and `set_workspace_project_root`, and — unlike `origin_uid` —
        // server-authoritative on pull: the cloud `codemux_workspaces`
        // schema now carries both, so `upsert_workspace_sync_from_server`
        // persists them from the server row.
        "ALTER TABLE workspaces_sync ADD COLUMN project_uid TEXT",
        "ALTER TABLE workspaces_sync ADD COLUMN workspace_kind TEXT",
        "ALTER TABLE workspaces_sync ADD COLUMN origin_path TEXT",
        // The repo's default branch as reported by the daemon poller
        // (`origin/HEAD` → main/master → current). Local-only column: it is
        // NOT sent to the cloud API (`ServerWorkspace`/`WorkspaceUpsertBody`
        // omit it), so it survives cloud pulls untouched and needs no
        // server-schema change. Lets a pull land a repo ROOT on the right
        // branch and protect it even when `git_branch` is null.
        "ALTER TABLE workspaces_sync ADD COLUMN default_branch TEXT",
        // Per-thread chat configuration (restart-resume follow-up).
        // Existing databases predate these columns; add them so a pane
        // reopened after a restart re-seeds the user's model / effort /
        // context-window / permission-mode and the auto-resume path can
        // rebuild the SDK session with the same settings.
        "ALTER TABLE agent_chat_sessions ADD COLUMN model TEXT",
        "ALTER TABLE agent_chat_sessions ADD COLUMN effort TEXT",
        "ALTER TABLE agent_chat_sessions ADD COLUMN context_window TEXT",
        "ALTER TABLE agent_chat_sessions ADD COLUMN permission_mode TEXT",
        "ALTER TABLE agent_chat_sessions ADD COLUMN fast_mode INTEGER",
        // Web-remote account mode (Stage A): how a session was admitted and,
        // for account-minted sessions, the verified Codemux account user id.
        // `source` defaults to 'pair' so every pre-existing (pairing-token) row
        // reads back as a paired device; account sessions set it to 'account'.
        "ALTER TABLE web_remote_sessions ADD COLUMN source TEXT NOT NULL DEFAULT 'pair'",
        "ALTER TABLE web_remote_sessions ADD COLUMN account_user_id TEXT",
        // Backfill permission_mode for rows created before the column above
        // existed. Those rows read back NULL, which makes backend
        // auto-resume rebuild the live session with no permissionMode (SDK
        // `default` mode → an approval prompt for every tool) even though
        // the frontend has always displayed the provider default ("Full
        // access") for them. Heal them to that same displayed default so the
        // UI and the rebuilt session agree. `provider` is stored lowercase
        // (see `upsert_agent_chat_session`); these strings mirror the
        // per-provider fallback in `commands::agent_chat::fallback_permission_mode`
        // and each provider's `default_permission_mode` in
        // `agent_provider/*/capabilities.rs`. OpenCode has no permission
        // modes, so its rows stay NULL. Ordered after the ALTER above so the
        // column exists; idempotent because after the first run no NULL rows
        // remain to update.
        "UPDATE agent_chat_sessions SET permission_mode = 'bypassPermissions' \
             WHERE permission_mode IS NULL AND provider = 'claude'",
        "UPDATE agent_chat_sessions SET permission_mode = 'danger-full-access' \
             WHERE permission_mode IS NULL AND provider = 'codex'",
        // v0.14.2's frontend null-mode fix used Claude's
        // `bypassPermissions` constant for every provider. A Codex launch
        // carrying that unsupported value omitted approvalPolicy/sandbox and
        // fell back to prompting while the picker displayed Full access.
        // Canonicalize already-persisted affected rows on upgrade. The reverse
        // mapping protects the symmetric stale-provider case as well.
        "UPDATE agent_chat_sessions SET permission_mode = 'danger-full-access' \
             WHERE permission_mode = 'bypassPermissions' AND provider = 'codex'",
        "UPDATE agent_chat_sessions SET permission_mode = 'bypassPermissions' \
             WHERE permission_mode = 'danger-full-access' AND provider = 'claude'",
        // Usage-ledger v12: reasoning split + cost provenance.
        "ALTER TABLE agent_usage_ledger ADD COLUMN reasoning_tokens INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE agent_usage_ledger ADD COLUMN cost_source TEXT",
        // Backfill provenance for rows written before the column existed.
        // Conservatively labels everything 'table' — including OpenCode
        // rows that were actually catalogue-priced, which cannot be
        // distinguished after the fact. It slightly understates the
        // "provider reported" share for pre-upgrade history and corrects
        // itself as new rows land.
        "UPDATE agent_usage_ledger SET cost_source = 'table' \
             WHERE cost_usd IS NOT NULL AND cost_source IS NULL",
        // Usage-ledger v13: CLI-log import provenance.
        "ALTER TABLE agent_usage_ledger ADD COLUMN source TEXT NOT NULL DEFAULT 'provider_history'",
        "ALTER TABLE agent_usage_ledger ADD COLUMN import_key TEXT",
        // Created HERE rather than in the CREATE batch above: on an
        // upgraded database `import_key` does not exist until the ALTER
        // immediately above runs, and a failed statement inside
        // `execute_batch` would abort every remaining CREATE with it.
        // Partial so the many live rows (import_key NULL) are not forced
        // to be distinct from one another.
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_usage_ledger_import_key \
             ON agent_usage_ledger(import_key) WHERE import_key IS NOT NULL",
    ] {
        if let Err(e) = conn.execute(stmt, []) {
            let msg = e.to_string();
            if !msg.contains("duplicate column name") {
                return Err(format!("Schema migration failed ({stmt}): {msg}"));
            }
        }
    }

    let installed_schema_version = conn
        .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
            row.get::<_, Option<u32>>(0)
        })
        .map_err(|e| format!("Failed to read installed schema version: {e}"))?
        .unwrap_or(0);

    // Upgrading users already have transcript rows when the v14 table first
    // appears. Backfill once during that migration; triggers maintain every
    // later insert/update/delete without a full transcript scan at startup.
    if installed_schema_version < 14 {
        conn.execute(
            "INSERT INTO agent_chat_search(rowid, content, thread_id, role, turn_id)
         SELECT
             m.id,
             CASE
                 WHEN json_extract(m.payload, '$.type') = 'user_message'
                     THEN json_extract(m.payload, '$.text')
                 ELSE json_extract(m.payload, '$.item.text')
             END,
             m.thread_id,
             CASE
                 WHEN json_extract(m.payload, '$.type') = 'user_message'
                     THEN 'user'
                 ELSE 'assistant'
             END,
             CASE
                 WHEN json_extract(m.payload, '$.type') = 'item_completed'
                     THEN json_extract(m.payload, '$.turn_id')
                 ELSE NULL
             END
         FROM agent_chat_messages m
         WHERE NOT EXISTS (
                   SELECT 1 FROM agent_chat_search f WHERE f.rowid = m.id
               )
           AND json_valid(m.payload)
           AND (
                 (
                     json_extract(m.payload, '$.type') = 'user_message'
                     AND length(trim(COALESCE(json_extract(m.payload, '$.text'), ''))) > 0
                 )
                 OR
                 (
                     json_extract(m.payload, '$.type') = 'item_completed'
                     AND json_extract(m.payload, '$.item.kind') = 'assistant_text'
                     AND json_extract(m.payload, '$.subagent_id') IS NULL
                     AND length(trim(COALESCE(json_extract(m.payload, '$.item.text'), ''))) > 0
                 )
           )",
            [],
        )
        .map_err(|e| format!("Failed to backfill agent chat search index: {e}"))?;
    }

    // Set (or advance) schema version. `IF NOT EXISTS` on every CREATE
    // above means re-running this on a v1 DB silently adds the new
    // table/indexes, so we just need to bump the stored version.
    let count: i32 = conn
        .query_row("SELECT COUNT(*) FROM schema_version", [], |row| row.get(0))
        .map_err(|e| format!("Failed to check schema version: {e}"))?;

    if count == 0 {
        conn.execute(
            "INSERT INTO schema_version (version) VALUES (?1)",
            params![SCHEMA_VERSION],
        )
        .map_err(|e| format!("Failed to set schema version: {e}"))?;
    } else {
        conn.execute(
            "UPDATE schema_version SET version = ?1",
            params![SCHEMA_VERSION],
        )
        .map_err(|e| format!("Failed to bump schema version: {e}"))?;
    }

    Ok(())
}

fn open_connection(path: &std::path::Path) -> Result<Connection, String> {
    let conn = Connection::open(path)
        .map_err(|e| format!("Failed to open database at {}: {e}", path.display()))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
        .map_err(|e| format!("Failed to set PRAGMA: {e}"))?;
    Ok(conn)
}

pub fn init_database() -> Result<DatabaseStore, String> {
    let db_path = database_path().ok_or("Could not determine config directory")?;

    // Ensure parent directory exists
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create config directory: {e}"))?;
    }

    let conn = open_connection(&db_path)?;
    create_schema(&conn)?;

    eprintln!(
        "[codemux::database] SQLite initialized at {}",
        db_path.display()
    );

    Ok(DatabaseStore {
        conn: Mutex::new(conn),
    })
}

/// Convert arbitrary palette input into a safe FTS5 prefix query. Punctuation
/// becomes a separator and repeated terms are removed, so quotes/operators in
/// user input can never change the MATCH expression's grammar.
fn conversation_fts_query(query: &str) -> Option<String> {
    let mut terms = Vec::new();
    let mut current = String::new();
    let mut seen = HashSet::new();

    let mut finish_term = |current: &mut String| {
        if current.is_empty() {
            return;
        }
        let term = std::mem::take(current);
        if seen.insert(term.to_lowercase()) {
            terms.push(format!("\"{term}\"*"));
        }
    };

    for character in query.chars() {
        if character.is_alphanumeric() || character == '_' {
            current.push(character);
        } else {
            finish_term(&mut current);
        }
    }
    finish_term(&mut current);

    (!terms.is_empty()).then(|| terms.join(" AND "))
}

impl DatabaseStore {
    /// Create an in-memory database (fallback if file DB fails, and for tests).
    pub fn new_in_memory() -> Self {
        let conn = Connection::open_in_memory().expect("Failed to open in-memory database");
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        create_schema(&conn).unwrap();
        Self {
            conn: Mutex::new(conn),
        }
    }
}

#[cfg(test)]
pub fn init_test_database() -> DatabaseStore {
    let conn = Connection::open_in_memory().expect("Failed to open in-memory database");
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    create_schema(&conn).unwrap();
    DatabaseStore {
        conn: Mutex::new(conn),
    }
}

// ── Settings ──

impl DatabaseStore {
    pub fn get_setting(&self, key: &str) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT value FROM settings WHERE user_id = 'local' AND key = ?1",
            params![key],
            |row| row.get(0),
        )
        .ok()
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO settings (user_id, key, value, updated_at) VALUES ('local', ?1, ?2, datetime('now'))
             ON CONFLICT(user_id, key) DO UPDATE SET value = ?2, updated_at = datetime('now')",
            params![key, value],
        )
        .map_err(|e| format!("Failed to set setting: {e}"))?;
        Ok(())
    }

    pub fn delete_setting(&self, key: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM settings WHERE user_id = 'local' AND key = ?1",
            params![key],
        )
        .map_err(|e| format!("Failed to delete setting: {e}"))?;
        Ok(())
    }

    pub fn get_all_settings(&self) -> HashMap<String, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT key, value FROM settings WHERE user_id = 'local'")
            .unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap();
        rows.filter_map(|r| r.ok()).collect()
    }

    // ── UI State ──

    pub fn get_ui_state(&self, key: &str) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT value FROM ui_state WHERE user_id = 'local' AND key = ?1",
            params![key],
            |row| row.get(0),
        )
        .ok()
    }

    pub fn set_ui_state(&self, key: &str, value: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO ui_state (user_id, key, value) VALUES ('local', ?1, ?2)
             ON CONFLICT(user_id, key) DO UPDATE SET value = ?2",
            params![key, value],
        )
        .map_err(|e| format!("Failed to set ui_state: {e}"))?;
        Ok(())
    }

    pub fn delete_ui_state(&self, key: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM ui_state WHERE user_id = 'local' AND key = ?1",
            params![key],
        )
        .map_err(|e| format!("Failed to delete ui_state: {e}"))?;
        Ok(())
    }

    // ── Project Scripts ──

    pub fn get_project_scripts(&self, project_root: &str) -> Option<ProjectScripts> {
        let key = format!("project.scripts:{project_root}");
        self.get_setting(&key)
            .and_then(|v| serde_json::from_str(&v).ok())
    }

    pub fn set_project_scripts(
        &self,
        project_root: &str,
        scripts: &ProjectScripts,
    ) -> Result<(), String> {
        let key = format!("project.scripts:{project_root}");
        let value = serde_json::to_string(scripts).map_err(|e| e.to_string())?;
        self.set_setting(&key, &value)
    }

    // ── Recent Projects ──

    pub fn add_recent_project(&self, path: &str, name: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO recent_projects (user_id, path, name, last_opened_at) VALUES ('local', ?1, ?2, datetime('now'))
             ON CONFLICT(user_id, path) DO UPDATE SET name = ?2, last_opened_at = datetime('now')",
            params![path, name],
        )
        .map_err(|e| format!("Failed to add recent project: {e}"))?;
        Ok(())
    }

    pub fn get_recent_projects(&self, limit: u32) -> Vec<RecentProject> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT path, name, last_opened_at FROM recent_projects
                 WHERE user_id = 'local' ORDER BY last_opened_at DESC, id DESC LIMIT ?1",
            )
            .unwrap();
        stmt.query_map(params![limit], |row| {
            Ok(RecentProject {
                path: row.get(0)?,
                name: row.get(1)?,
                last_opened_at: row.get(2)?,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }
}

// ── Hosts (Step 2 of cloud push) ──
//
// CRUD over the `hosts` table. Soft-delete semantics: `delete_host`
// stamps `deleted_at` rather than removing the row, so the sync layer
// has a tombstone to push. `purge_synced_deletes` is called by the
// sync layer after a successful round-trip to physically remove
// already-acknowledged tombstones.
//
// SSH credentials live in `~/.ssh/`, never here. This table holds
// identity only.

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HostRecord {
    pub id: i64,
    pub server_id: Option<String>,
    pub name: String,
    pub ssh_target: String,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
    pub dirty: bool,
    /// Local-only: RFC 3339 of the last tick on which this install
    /// reached the host over SSH. Never synced.
    pub last_seen_at: Option<String>,
    /// Local-only: last workspace-disk measurement the host reported,
    /// and when. Never synced.
    pub disk_bytes: Option<u64>,
    pub disk_measured_at: Option<String>,
}

/// Column list every `HostRecord` SELECT must use, in `row_to_host` order.
const HOST_COLUMNS: &str = "id, server_id, name, ssh_target, created_at, updated_at, deleted_at, dirty, \
     last_seen_at, disk_bytes, disk_measured_at";

impl DatabaseStore {
    /// Insert a new host. Marked dirty so the next sync round-trip pushes it.
    pub fn insert_host(&self, name: &str, ssh_target: &str) -> Result<HostRecord, String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO hosts (user_id, name, ssh_target, dirty)
             VALUES ('local', ?1, ?2, 1)",
            params![name, ssh_target],
        )
        .map_err(|e| format!("Failed to insert host: {e}"))?;
        let id = conn.last_insert_rowid();
        conn.query_row(
            &format!("SELECT {HOST_COLUMNS}
             FROM hosts WHERE id = ?1"),
            params![id],
            row_to_host,
        )
        .map_err(|e| format!("Failed to re-read inserted host: {e}"))
    }

    /// Return all non-deleted hosts for the local user.
    pub fn list_hosts(&self) -> Vec<HostRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            &format!("SELECT {HOST_COLUMNS}
             FROM hosts
             WHERE user_id = 'local' AND deleted_at IS NULL
             ORDER BY name COLLATE NOCASE ASC"),
        ) {
            Ok(s) => s,
            Err(error) => {
                eprintln!("[codemux::database] list_hosts prepare failed: {error}");
                return Vec::new();
            }
        };
        let rows = match stmt.query_map([], row_to_host) {
            Ok(r) => r,
            Err(error) => {
                eprintln!("[codemux::database] list_hosts query_map failed: {error}");
                return Vec::new();
            }
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    /// Return every host row including soft-deleted tombstones — used
    /// by the sync layer to push pending deletions to the server.
    pub fn list_hosts_for_sync(&self) -> Vec<HostRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            &format!("SELECT {HOST_COLUMNS}
             FROM hosts WHERE user_id = 'local'"),
        ) {
            Ok(s) => s,
            Err(error) => {
                eprintln!("[codemux::database] list_hosts_for_sync prepare failed: {error}");
                return Vec::new();
            }
        };
        let rows = match stmt.query_map([], row_to_host) {
            Ok(r) => r,
            Err(error) => {
                eprintln!("[codemux::database] list_hosts_for_sync query_map failed: {error}");
                return Vec::new();
            }
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    /// Return only rows with unpushed changes (dirty=1). Used by the
    /// sync layer's "push my deltas" step so we don't re-upload rows
    /// that already match the server.
    pub fn list_dirty_hosts(&self) -> Vec<HostRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            &format!("SELECT {HOST_COLUMNS}
             FROM hosts WHERE user_id = 'local' AND dirty = 1"),
        ) {
            Ok(s) => s,
            Err(error) => {
                eprintln!("[codemux::database] list_dirty_hosts prepare failed: {error}");
                return Vec::new();
            }
        };
        let rows = match stmt.query_map([], row_to_host) {
            Ok(r) => r,
            Err(error) => {
                eprintln!("[codemux::database] list_dirty_hosts query_map failed: {error}");
                return Vec::new();
            }
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    pub fn update_host(&self, id: i64, name: &str, ssh_target: &str) -> Result<HostRecord, String> {
        let conn = self.conn.lock().unwrap();
        let affected = conn
            .execute(
                "UPDATE hosts
                 SET name = ?1, ssh_target = ?2, updated_at = datetime('now'), dirty = 1
                 WHERE id = ?3 AND user_id = 'local' AND deleted_at IS NULL",
                params![name, ssh_target, id],
            )
            .map_err(|e| format!("Failed to update host: {e}"))?;
        if affected == 0 {
            return Err(format!("No host with id {id}"));
        }
        conn.query_row(
            &format!("SELECT {HOST_COLUMNS}
             FROM hosts WHERE id = ?1"),
            params![id],
            row_to_host,
        )
        .map_err(|e| format!("Failed to re-read updated host: {e}"))
    }

    /// Soft-delete: stamp `deleted_at` and mark dirty so the next sync
    /// pushes the tombstone. The row stays in the DB until
    /// `purge_synced_deletes` runs.
    pub fn delete_host(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let affected = conn
            .execute(
                "UPDATE hosts
                 SET deleted_at = datetime('now'), updated_at = datetime('now'), dirty = 1
                 WHERE id = ?1 AND user_id = 'local' AND deleted_at IS NULL",
                params![id],
            )
            .map_err(|e| format!("Failed to soft-delete host: {e}"))?;
        if affected == 0 {
            return Err(format!("No host with id {id}"));
        }
        Ok(())
    }

    /// Stamp the local-only observation columns after a tick on which
    /// the host answered. `disk_bytes` is `Some` only when the host
    /// reported a fresh measurement; `None` leaves the previous figure
    /// and its timestamp alone. Deliberately touches neither `dirty` nor
    /// `updated_at`: these columns are this install's private view and
    /// must not trigger a sync push.
    pub fn record_host_seen(
        &self,
        id: i64,
        seen_at: &str,
        disk_bytes: Option<u64>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE hosts
             SET last_seen_at = ?1,
                 disk_bytes = COALESCE(?2, disk_bytes),
                 disk_measured_at = CASE WHEN ?2 IS NULL THEN disk_measured_at ELSE ?1 END
             WHERE id = ?3",
            params![seen_at, disk_bytes.map(|b| b as i64), id],
        )
        .map_err(|e| format!("Failed to record host observation: {e}"))?;
        Ok(())
    }

    /// Clear the dirty flag on a host after a successful push. Optionally
    /// stamp `server_id` if this was the first upload.
    pub fn mark_host_synced(&self, id: i64, server_id: Option<&str>) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        if let Some(sid) = server_id {
            conn.execute(
                "UPDATE hosts SET dirty = 0, server_id = ?1 WHERE id = ?2",
                params![sid, id],
            )
            .map_err(|e| format!("Failed to mark host synced: {e}"))?;
        } else {
            conn.execute("UPDATE hosts SET dirty = 0 WHERE id = ?1", params![id])
                .map_err(|e| format!("Failed to mark host synced: {e}"))?;
        }
        Ok(())
    }

    /// Hard-delete tombstones the server has confirmed it removed. Safe
    /// to call after a successful sync round-trip; no-op when nothing
    /// matches.
    pub fn purge_acknowledged_deletes(&self) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM hosts WHERE deleted_at IS NOT NULL AND dirty = 0",
            [],
        )
        .map_err(|e| format!("Failed to purge tombstones: {e}"))?;
        Ok(())
    }

    /// Upsert a row received from the server. If a local row already
    /// exists with the same `server_id`, update in place; otherwise
    /// insert. Always marked `dirty = 0` because this row came from the
    /// server. The UPDATE names its columns so the local-only
    /// observation columns (`last_seen_at`, `disk_bytes`,
    /// `disk_measured_at`) survive every pull.
    pub fn upsert_host_from_server(
        &self,
        server_id: &str,
        name: &str,
        ssh_target: &str,
        created_at: &str,
        updated_at: &str,
        deleted_at: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        // Try to update an existing row first.
        let updated = conn
            .execute(
                "UPDATE hosts
                 SET name = ?1, ssh_target = ?2, created_at = ?3, updated_at = ?4,
                     deleted_at = ?5, dirty = 0
                 WHERE user_id = 'local' AND server_id = ?6",
                params![name, ssh_target, created_at, updated_at, deleted_at, server_id],
            )
            .map_err(|e| format!("Failed to update host from server: {e}"))?;
        if updated == 0 {
            conn.execute(
                "INSERT INTO hosts (user_id, server_id, name, ssh_target, created_at, updated_at, deleted_at, dirty)
                 VALUES ('local', ?1, ?2, ?3, ?4, ?5, ?6, 0)",
                params![server_id, name, ssh_target, created_at, updated_at, deleted_at],
            )
            .map_err(|e| format!("Failed to insert host from server: {e}"))?;
        }
        Ok(())
    }
}

fn row_to_host(row: &rusqlite::Row<'_>) -> rusqlite::Result<HostRecord> {
    let dirty_int: i64 = row.get(7)?;
    Ok(HostRecord {
        id: row.get(0)?,
        server_id: row.get(1)?,
        name: row.get(2)?,
        ssh_target: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        deleted_at: row.get(6)?,
        dirty: dirty_int != 0,
        last_seen_at: row.get(8)?,
        disk_bytes: row
            .get::<_, Option<i64>>(9)?
            .and_then(|b| u64::try_from(b).ok()),
        disk_measured_at: row.get(10)?,
    })
}

// ─── Workspaces sync mirror (cross-device workspace registry) ────
//
// This is the local-side mirror of the server-side `codemux_workspaces`
// table. It is *separate* from `workspace_state` because the two
// concerns are different: `workspace_state` holds per-device runtime
// UI state (tab order, collapse, last_active_at); `workspaces_sync`
// holds the synced identity (title, host, project, branch) that
// crosses the account-wide network boundary.
//
// `workspace_id` is the local id from `next_id` when this row was
// either created locally OR adopted from a sync-pulled row. NULL
// means the row was pulled but not yet adopted (the user hasn't
// clicked "Pull to this device" on it).
//
// Lookup paths the sync layer + the UI need:
// - by local workspace_id (after a local mutation, find the synced row to mark dirty)
// - by server_id (after a sync pull, upsert the matching local row)
// - dirty list (push only rows that changed since last sync)
// - all-non-deleted (overview UI render)
//
// The shape mirrors `hosts` 1:1 so the sync client can mostly copy
// the existing pattern.

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceSyncRecord {
    pub id: i64,
    pub server_id: Option<String>,
    /// Local workspace_id (the `WorkspaceSnapshot.workspace_id` value).
    /// None when the row was pulled from a sibling device and we have
    /// not adopted it locally yet.
    pub workspace_id: Option<String>,
    pub title: String,
    /// Server-side host id (matches `hosts.server_id`). None = local
    /// to the device that authored the row.
    pub host_server_id: Option<String>,
    pub project_path: Option<String>,
    pub project_remote: Option<String>,
    pub git_branch: Option<String>,
    /// Workspace git HEAD sha at the last reconcile. Phase-4
    /// divergence detection compares this across devices: when the
    /// same (project_remote, git_branch) pair has different head
    /// shas on multiple devices, the overview surfaces a warning
    /// chip. None for new rows that haven't been reconciled yet,
    /// or for rows whose worktree had no commits.
    pub git_head_sha: Option<String>,
    /// Stable UUID assigned by the remote daemon
    /// (`remote::workspace::Workspace.id`) when the desktop's
    /// host-inventory poller discovered this row. Lets the poller's
    /// reconcile step recognise the same remote workspace across
    /// repeated polls — without it, every poll would create a fresh
    /// sync row. Always `None` on rows that originated on this device
    /// or arrived purely via cloud pull (the cloud schema does not
    /// carry `origin_uid` today).
    pub origin_uid: Option<String>,
    /// Deterministic project identity (`UUIDv5`), see
    /// `crate::project_identity`. Local-only column today: set by the
    /// host-inventory poller from the daemon registry; null for rows
    /// that arrived purely via cloud pull or originated locally before
    /// Phase 2 stamps `WorkspaceSnapshot`.
    pub project_uid: Option<String>,
    /// `"main"` | `"worktree"`. Local-only column, same lifecycle as
    /// `project_uid`.
    pub workspace_kind: Option<String>,
    /// The workspace's ACTUAL absolute path on the originating host, as
    /// reported by the remote daemon (`remote::workspace::Workspace.path`).
    /// Set only by the host-inventory poller for remote-discovered rows;
    /// null otherwise. This is the authoritative rsync source for pulling
    /// an agent-created workspace back — unlike `project_path` (which is
    /// the project root, not the worktree dir) and unlike the reconstructed
    /// `~/.codemux/worktrees/<project>/<branch>` convention (which only
    /// matches workspaces the desktop *pushed*, not ones the agent created
    /// at an arbitrary path on the host). Local-only column, same
    /// lifecycle as `origin_uid`; survives cloud pulls untouched.
    pub origin_path: Option<String>,
    /// The repo's default branch from the daemon poller (`origin/HEAD` →
    /// main/master → current). Local-only column, same lifecycle as
    /// `origin_path` — set by the host-inventory poller, NOT sent to the
    /// cloud, survives cloud pulls untouched. Lets a pull land a repo ROOT
    /// on the right branch and protect it even when `git_branch` is null.
    pub default_branch: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
    pub dirty: bool,
}

impl DatabaseStore {
    /// Insert a freshly-created local workspace into the sync mirror.
    /// Marked dirty so the next push uploads it. Returns the row so
    /// callers can stash the assigned local sync `id` if useful.
    pub fn insert_workspace_sync(
        &self,
        workspace_id: &str,
        title: &str,
        host_server_id: Option<&str>,
        project_path: Option<&str>,
        project_remote: Option<&str>,
        git_branch: Option<&str>,
        git_head_sha: Option<&str>,
        project_uid: Option<&str>,
        workspace_kind: Option<&str>,
    ) -> Result<WorkspaceSyncRecord, String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO workspaces_sync
                (user_id, workspace_id, title, host_server_id,
                 project_path, project_remote, git_branch, git_head_sha,
                 project_uid, workspace_kind, dirty)
             VALUES ('local', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1)",
            params![
                workspace_id,
                title,
                host_server_id,
                project_path,
                project_remote,
                git_branch,
                git_head_sha,
                project_uid,
                workspace_kind,
            ],
        )
        .map_err(|e| format!("Failed to insert workspace sync row: {e}"))?;
        let id = conn.last_insert_rowid();
        conn.query_row(
            "SELECT id, server_id, workspace_id, title, host_server_id,
                    project_path, project_remote, git_branch, git_head_sha,
                    created_at, updated_at, deleted_at, dirty, origin_uid, project_uid, workspace_kind, origin_path, default_branch
             FROM workspaces_sync WHERE id = ?1",
            params![id],
            row_to_workspace_sync,
        )
        .map_err(|e| format!("Failed to re-read inserted workspace sync row: {e}"))
    }

    /// Update a workspace sync row keyed by the local `workspace_id`.
    /// Bumps `updated_at` and sets `dirty = 1`. No-op if the row is
    /// already soft-deleted (matching the hosts/automations pattern).
    pub fn update_workspace_sync_by_workspace_id(
        &self,
        workspace_id: &str,
        title: &str,
        host_server_id: Option<&str>,
        project_path: Option<&str>,
        project_remote: Option<&str>,
        git_branch: Option<&str>,
        git_head_sha: Option<&str>,
        project_uid: Option<&str>,
        workspace_kind: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE workspaces_sync
             SET title = ?1, host_server_id = ?2, project_path = ?3,
                 project_remote = ?4, git_branch = ?5, git_head_sha = ?6,
                 project_uid = ?7, workspace_kind = ?8,
                 updated_at = datetime('now'), dirty = 1
             WHERE user_id = 'local' AND workspace_id = ?9
               AND deleted_at IS NULL",
            params![
                title,
                host_server_id,
                project_path,
                project_remote,
                git_branch,
                git_head_sha,
                project_uid,
                workspace_kind,
                workspace_id,
            ],
        )
        .map_err(|e| format!("Failed to update workspace sync row: {e}"))?;
        Ok(())
    }

    /// Soft-delete a workspace sync row by local workspace_id. Sets
    /// `deleted_at = now`, `updated_at = now`, `dirty = 1`. The next
    /// push DELETEs the server row; `purge_acknowledged_workspace_sync_deletes`
    /// then hard-deletes the local tombstone.
    pub fn soft_delete_workspace_sync_by_workspace_id(
        &self,
        workspace_id: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE workspaces_sync
             SET deleted_at = datetime('now'),
                 updated_at = datetime('now'), dirty = 1
             WHERE user_id = 'local' AND workspace_id = ?1
               AND deleted_at IS NULL",
            params![workspace_id],
        )
        .map_err(|e| format!("Failed to soft-delete workspace sync row: {e}"))?;
        Ok(())
    }

    /// List every non-deleted workspace sync row (live + pulled-but-
    /// unadopted). UI uses this to render the overview, including
    /// workspaces that only exist on other devices.
    pub fn list_workspaces_sync(&self) -> Vec<WorkspaceSyncRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT id, server_id, workspace_id, title, host_server_id,
                    project_path, project_remote, git_branch, git_head_sha,
                    created_at, updated_at, deleted_at, dirty, origin_uid, project_uid, workspace_kind, origin_path, default_branch
             FROM workspaces_sync
             WHERE user_id = 'local' AND deleted_at IS NULL
             ORDER BY updated_at DESC",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map([], row_to_workspace_sync)
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// List EVERY row including tombstones — used by the pull sweep
    /// to detect rows whose server_id is no longer in the API response.
    pub fn list_workspaces_sync_for_sync(&self) -> Vec<WorkspaceSyncRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT id, server_id, workspace_id, title, host_server_id,
                    project_path, project_remote, git_branch, git_head_sha,
                    created_at, updated_at, deleted_at, dirty, origin_uid, project_uid, workspace_kind, origin_path, default_branch
             FROM workspaces_sync
             WHERE user_id = 'local'",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map([], row_to_workspace_sync)
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// List only `dirty = 1` rows — the push loop walks this.
    pub fn list_dirty_workspaces_sync(&self) -> Vec<WorkspaceSyncRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT id, server_id, workspace_id, title, host_server_id,
                    project_path, project_remote, git_branch, git_head_sha,
                    created_at, updated_at, deleted_at, dirty, origin_uid, project_uid, workspace_kind, origin_path, default_branch
             FROM workspaces_sync
             WHERE user_id = 'local' AND dirty = 1",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map([], row_to_workspace_sync)
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// Clear dirty after a successful push. Optionally stamp the
    /// server_id (first push only). Mirrors `mark_host_synced`.
    pub fn mark_workspace_sync_synced(
        &self,
        id: i64,
        server_id: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        if let Some(sid) = server_id {
            conn.execute(
                "UPDATE workspaces_sync SET dirty = 0, server_id = ?1 WHERE id = ?2",
                params![sid, id],
            )
        } else {
            conn.execute(
                "UPDATE workspaces_sync SET dirty = 0 WHERE id = ?1",
                params![id],
            )
        }
        .map_err(|e| format!("Failed to mark workspace sync row synced: {e}"))?;
        Ok(())
    }

    /// Hard-delete tombstones the server has acknowledged. Called at
    /// the end of every push pass.
    pub fn purge_acknowledged_workspace_sync_deletes(&self) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM workspaces_sync
             WHERE deleted_at IS NOT NULL AND dirty = 0",
            [],
        )
        .map_err(|e| format!("Failed to purge workspace sync tombstones: {e}"))?;
        Ok(())
    }

    /// Idempotent upsert from a server pull. Always sets `dirty = 0`.
    /// Does NOT clobber the local `workspace_id` if one is already
    /// stamped — adoption is sticky.
    pub fn upsert_workspace_sync_from_server(
        &self,
        server_id: &str,
        title: &str,
        host_server_id: Option<&str>,
        project_path: Option<&str>,
        project_remote: Option<&str>,
        git_branch: Option<&str>,
        git_head_sha: Option<&str>,
        created_at: &str,
        updated_at: &str,
        deleted_at: Option<&str>,
        project_uid: Option<&str>,
        workspace_kind: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let updated = conn
            .execute(
                // `project_uid`/`workspace_kind` are server-authoritative
                // ONLY when the server actually has a value. A null from
                // the cloud must never clobber a known-good local identity
                // that the host-inventory poller derived (the cloud schema
                // may carry null for older rows). COALESCE(server, local)
                // keeps the local value when the server's is null.
                "UPDATE workspaces_sync
                 SET title = ?1, host_server_id = ?2, project_path = ?3,
                     project_remote = ?4, git_branch = ?5, git_head_sha = ?6,
                     created_at = ?7, updated_at = ?8,
                     deleted_at = ?9,
                     project_uid = COALESCE(?11, project_uid),
                     workspace_kind = COALESCE(?12, workspace_kind),
                     dirty = 0
                 WHERE user_id = 'local' AND server_id = ?10",
                params![
                    title,
                    host_server_id,
                    project_path,
                    project_remote,
                    git_branch,
                    git_head_sha,
                    created_at,
                    updated_at,
                    deleted_at,
                    server_id,
                    project_uid,
                    workspace_kind,
                ],
            )
            .map_err(|e| format!("Failed to update workspace sync from server: {e}"))?;
        if updated == 0 {
            conn.execute(
                "INSERT INTO workspaces_sync
                    (user_id, server_id, workspace_id, title, host_server_id,
                     project_path, project_remote, git_branch, git_head_sha,
                     created_at, updated_at, deleted_at,
                     project_uid, workspace_kind, dirty)
                 VALUES ('local', ?1, NULL, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                         ?11, ?12, 0)",
                params![
                    server_id,
                    title,
                    host_server_id,
                    project_path,
                    project_remote,
                    git_branch,
                    git_head_sha,
                    created_at,
                    updated_at,
                    deleted_at,
                    project_uid,
                    workspace_kind,
                ],
            )
            .map_err(|e| format!("Failed to insert workspace sync from server: {e}"))?;
        }
        Ok(())
    }

    /// Stamp a local workspace_id onto a previously-pulled row (used
    /// when adopting a synced workspace into this device's app_state).
    pub fn link_workspace_sync_to_local(
        &self,
        server_id: &str,
        workspace_id: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE workspaces_sync
             SET workspace_id = ?1, updated_at = datetime('now')
             WHERE user_id = 'local' AND server_id = ?2",
            params![workspace_id, server_id],
        )
        .map_err(|e| format!("Failed to link workspace sync row to local: {e}"))?;
        Ok(())
    }

    /// Revert a sync row back to a sibling/remote-only row by clearing
    /// its local `workspace_id` link (set NULL). Used to ROLL BACK an
    /// optimistic adoption whose pull failed: the row must become a
    /// re-pullable sibling again rather than stay linked to a shell
    /// that was torn down — otherwise `reconcile_from_snapshot` would
    /// see a linked row with no live workspace and soft-delete
    /// (tombstone) it, making the workspace vanish from the overview
    /// instead of staying available to retry. We deliberately do NOT
    /// mark the row dirty: clearing a local-only link is not a change
    /// the cloud needs to hear about (the row's server_id/identity is
    /// unchanged), and the next inventory poll keeps it fresh.
    pub fn unlink_workspace_sync_from_local(&self, server_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE workspaces_sync
             SET workspace_id = NULL, updated_at = datetime('now')
             WHERE user_id = 'local' AND server_id = ?1",
            params![server_id],
        )
        .map_err(|e| format!("Failed to unlink workspace sync row: {e}"))?;
        Ok(())
    }

    // ── Host-inventory auto-publish helpers ─────────────────────────
    //
    // The desktop's host-inventory poller (see `hosts_inventory.rs`)
    // periodically SSHes every configured host, fetches the remote
    // daemon's workspace list, and reconciles it into the local
    // `workspaces_sync` table as sibling-only rows (no local
    // `workspace_id`, dirty=1) so the next `push()` tick uploads them
    // to the cloud and other devices see them in the overview.
    //
    // Identity contract for these rows:
    //
    // - `host_server_id` = the configured host's `server_id` (the
    //   stable cross-device host identity). Required.
    // - `origin_uid`     = `remote::workspace::Workspace.id` (a UUID
    //   assigned by the host's daemon at workspace-create time).
    //   Required. Lets repeated polls update-in-place instead of
    //   creating duplicate rows.
    // - `workspace_id`   = NULL until the user adopts the row via
    //   "Pull to this device".
    //
    // The cloud schema does not carry `origin_uid` today; it stays
    // local-only. Cross-device dedupe (two laptops both polling the
    // same host) is best-effort by `find_remote_discovered_by_origin`
    // alone — if Device B has not yet pulled the row Device A
    // published, B may briefly create a parallel row that converges
    // on the next pull cycle. Acceptable for v1; tracked in the docs.

    /// Find a remote-discovered sync row by `(host_server_id,
    /// origin_uid)`. Used by the inventory reconcile to decide
    /// insert-vs-update-in-place per poll tick.
    ///
    /// Returns `None` if no row with that pair exists, including the
    /// case where a row exists with the same `origin_uid` but a
    /// different `host_server_id` (the same UUID on two different
    /// hosts must be treated as two distinct workspaces — UUIDs are
    /// only unique within one host's registry, never assumed unique
    /// across hosts).
    pub fn find_workspace_sync_by_host_and_origin_uid(
        &self,
        host_server_id: &str,
        origin_uid: &str,
    ) -> Option<WorkspaceSyncRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, server_id, workspace_id, title, host_server_id,
                        project_path, project_remote, git_branch, git_head_sha,
                        created_at, updated_at, deleted_at, dirty, origin_uid,
                        project_uid, workspace_kind, origin_path, default_branch
                 FROM workspaces_sync
                 WHERE user_id = 'local'
                   AND host_server_id = ?1
                   AND origin_uid = ?2
                   AND deleted_at IS NULL
                 LIMIT 1",
            )
            .ok()?;
        stmt.query_row(params![host_server_id, origin_uid], row_to_workspace_sync)
            .ok()
    }

    /// Insert a sibling-only sync row discovered by polling a host's
    /// inventory. `workspace_id` is intentionally NULL — the row is
    /// only adopted as a local workspace when the user clicks "Pull
    /// to this device". `dirty=1` so the next `push()` tick uploads
    /// it to the cloud and other devices of the same account see it.
    ///
    /// `host_server_id` and `origin_uid` together identify the row
    /// uniquely on this device (see `find_workspace_sync_by_host_and_origin_uid`).
    pub fn insert_remote_discovered_workspace_sync(
        &self,
        host_server_id: &str,
        origin_uid: &str,
        title: &str,
        project_path: Option<&str>,
        project_remote: Option<&str>,
        git_branch: Option<&str>,
        project_uid: Option<&str>,
        workspace_kind: Option<&str>,
        origin_path: Option<&str>,
        default_branch: Option<&str>,
    ) -> Result<WorkspaceSyncRecord, String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO workspaces_sync
                (user_id, workspace_id, title, host_server_id,
                 project_path, project_remote, git_branch, origin_uid,
                 project_uid, workspace_kind, origin_path, default_branch, dirty)
             VALUES ('local', NULL, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1)",
            params![
                title,
                host_server_id,
                project_path,
                project_remote,
                git_branch,
                origin_uid,
                project_uid,
                workspace_kind,
                origin_path,
                default_branch,
            ],
        )
        .map_err(|e| format!("Failed to insert remote-discovered workspace sync row: {e}"))?;
        let id = conn.last_insert_rowid();
        conn.query_row(
            "SELECT id, server_id, workspace_id, title, host_server_id,
                    project_path, project_remote, git_branch, git_head_sha,
                    created_at, updated_at, deleted_at, dirty, origin_uid, project_uid, workspace_kind, origin_path, default_branch
             FROM workspaces_sync WHERE id = ?1",
            params![id],
            row_to_workspace_sync,
        )
        .map_err(|e| format!("Failed to re-read inserted remote-discovered row: {e}"))
    }

    /// Update mutable fields of a remote-discovered row (matched by
    /// the row's primary `id`). Bumps `updated_at` and marks
    /// `dirty=1` so the next push propagates the change. No-op on
    /// soft-deleted rows, matching `update_workspace_sync_by_workspace_id`.
    ///
    /// Note we deliberately do NOT touch `host_server_id` or
    /// `origin_uid` — those define the row's identity and must be
    /// stable across reconciles.
    pub fn update_remote_discovered_workspace_sync(
        &self,
        id: i64,
        title: &str,
        project_path: Option<&str>,
        project_remote: Option<&str>,
        git_branch: Option<&str>,
        project_uid: Option<&str>,
        workspace_kind: Option<&str>,
        origin_path: Option<&str>,
        default_branch: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE workspaces_sync
             SET title = ?1, project_path = ?2,
                 project_remote = ?3, git_branch = ?4,
                 project_uid = ?5, workspace_kind = ?6,
                 origin_path = ?8, default_branch = ?9,
                 updated_at = datetime('now'), dirty = 1
             WHERE id = ?7 AND deleted_at IS NULL",
            params![
                title,
                project_path,
                project_remote,
                git_branch,
                project_uid,
                workspace_kind,
                id,
                origin_path,
                default_branch,
            ],
        )
        .map_err(|e| format!("Failed to update remote-discovered row: {e}"))?;
        Ok(())
    }

    /// Find a remote-discovered row for `(host_server_id, origin_uid)`
    /// that is SOFT-DELETED (a tombstone). Used only by the inventory
    /// reconcile's undelete-on-reappear path: when a host workspace that
    /// was previously adopted-then-closed (its row tombstoned by the
    /// close-path reconcile) shows up again in a fresh poll, we resurrect
    /// the SAME row instead of inserting a duplicate — so the cloud
    /// `server_id` (cross-device identity) survives the close/reopen
    /// round-trip and other devices don't see the workspace vanish then
    /// reappear as a brand-new row. If multiple tombstones exist, the
    /// most recent by id wins.
    pub fn find_remote_discovered_tombstone(
        &self,
        host_server_id: &str,
        origin_uid: &str,
    ) -> Option<WorkspaceSyncRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, server_id, workspace_id, title, host_server_id,
                        project_path, project_remote, git_branch, git_head_sha,
                        created_at, updated_at, deleted_at, dirty, origin_uid,
                        project_uid, workspace_kind, origin_path, default_branch
                 FROM workspaces_sync
                 WHERE user_id = 'local'
                   AND host_server_id = ?1
                   AND origin_uid = ?2
                   AND deleted_at IS NOT NULL
                 ORDER BY id DESC
                 LIMIT 1",
            )
            .ok()?;
        stmt.query_row(params![host_server_id, origin_uid], row_to_workspace_sync)
            .ok()
    }

    /// Resurrect a soft-deleted remote-discovered row (matched by primary
    /// `id`): clear `deleted_at`, ALSO clear any stale `workspace_id`
    /// link, refresh the mutable fields from the latest inventory, and
    /// mark `dirty=1`.
    ///
    /// Clearing `workspace_id` is essential: the tombstone may have been
    /// created by closing an *adopted* workspace, so it still points at a
    /// local id that no longer exists. Reviving that link would recreate
    /// the exact "linked row with no live workspace" orphan that produces
    /// the phantom "you already have this branch open" pull-conflict. A
    /// resurrected row must be a clean, re-pullable sibling
    /// (`workspace_id IS NULL`). Marking dirty makes the next push re-assert
    /// the row to the cloud as a PATCH on the surviving `server_id` rather
    /// than a churny DELETE-then-POST.
    pub fn undelete_remote_discovered_workspace_sync(
        &self,
        id: i64,
        title: &str,
        project_path: Option<&str>,
        project_remote: Option<&str>,
        git_branch: Option<&str>,
        project_uid: Option<&str>,
        workspace_kind: Option<&str>,
        origin_path: Option<&str>,
        default_branch: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE workspaces_sync
             SET deleted_at = NULL, workspace_id = NULL,
                 title = ?1, project_path = ?2, project_remote = ?3,
                 git_branch = ?4, project_uid = ?5, workspace_kind = ?6,
                 origin_path = ?8, default_branch = ?9,
                 updated_at = datetime('now'), dirty = 1
             WHERE id = ?7",
            params![
                title,
                project_path,
                project_remote,
                git_branch,
                project_uid,
                workspace_kind,
                id,
                origin_path,
                default_branch,
            ],
        )
        .map_err(|e| format!("Failed to undelete remote-discovered row: {e}"))?;
        Ok(())
    }

    /// List every non-deleted remote-discovered row for a host. Used
    /// by the inventory reconcile pass to compute the
    /// "disappeared from the host" set: any row in this list whose
    /// `origin_uid` is no longer in the host's current inventory must
    /// be soft-deleted so the cloud row goes away on the next push.
    pub fn list_remote_discovered_for_host(
        &self,
        host_server_id: &str,
    ) -> Vec<WorkspaceSyncRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT id, server_id, workspace_id, title, host_server_id,
                    project_path, project_remote, git_branch, git_head_sha,
                    created_at, updated_at, deleted_at, dirty, origin_uid, project_uid, workspace_kind, origin_path, default_branch
             FROM workspaces_sync
             WHERE user_id = 'local'
               AND host_server_id = ?1
               AND origin_uid IS NOT NULL
               AND deleted_at IS NULL",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map(params![host_server_id], row_to_workspace_sync)
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// Soft-delete a remote-discovered row by primary `id`. Sets
    /// `deleted_at` + `dirty=1` so the next push DELETEs the cloud
    /// row. Unlike `soft_delete_workspace_sync_by_workspace_id`,
    /// this targets the row's primary key directly because
    /// remote-discovered rows have `workspace_id IS NULL`.
    pub fn soft_delete_remote_discovered_workspace_sync_by_id(
        &self,
        id: i64,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE workspaces_sync
             SET deleted_at = datetime('now'),
                 updated_at = datetime('now'),
                 dirty = 1
             WHERE id = ?1 AND deleted_at IS NULL",
            params![id],
        )
        .map_err(|e| format!("Failed to soft-delete remote-discovered row: {e}"))?;
        Ok(())
    }
}

fn row_to_workspace_sync(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkspaceSyncRecord> {
    let dirty_int: i64 = row.get(12)?;
    Ok(WorkspaceSyncRecord {
        id: row.get(0)?,
        server_id: row.get(1)?,
        workspace_id: row.get(2)?,
        title: row.get(3)?,
        host_server_id: row.get(4)?,
        project_path: row.get(5)?,
        project_remote: row.get(6)?,
        git_branch: row.get(7)?,
        git_head_sha: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
        deleted_at: row.get(11)?,
        dirty: dirty_int != 0,
        origin_uid: row.get(13)?,
        project_uid: row.get(14)?,
        workspace_kind: row.get(15)?,
        origin_path: row.get(16)?,
        default_branch: row.get(17)?,
    })
}

// ── Automations ──
//
// CRUD over the `automations` and `automation_runs` tables. Automations
// reuse the soft-delete + `dirty` flag convention from `hosts` so the
// account-sync layer can be bolted on later without a migration. Run
// rows are append-only history and are never pruned here — only the
// agent worktrees on the host are reclaimed, by the host scheduler.

/// One scheduled automation: a named prompt + agent + recurrence that
/// fires on a chosen host.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AutomationRecord {
    pub id: i64,
    pub server_id: Option<String>,
    pub name: String,
    pub prompt: String,
    pub agent: String,
    /// Complete iCalendar recurrence block (DTSTART + RRULE, RFC 5545).
    pub schedule: String,
    /// IANA timezone name, for display.
    pub timezone: String,
    /// Target host row id, or `None` when not yet assigned.
    pub host_id: Option<i64>,
    /// Local path of the project repository (valid on the machine that
    /// created the automation).
    pub project_path: Option<String>,
    /// The project's git remote URL — how a host that lacks
    /// `project_path` obtains the repo (it clones this).
    pub project_remote: Option<String>,
    pub enabled: bool,
    /// How many completed run worktrees the host keeps before pruning.
    pub retention_limit: i64,
    pub last_run_at: Option<String>,
    pub next_run_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
    pub dirty: bool,
}

/// One fire of an automation (or a skipped fire).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AutomationRunRecord {
    pub id: i64,
    pub automation_id: i64,
    /// `scheduled` | `running` | `succeeded` | `failed`
    /// | `skipped_offline` | `skipped_busy`.
    pub status: String,
    pub scheduled_for: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub host_id: Option<i64>,
    pub workspace_id: Option<String>,
    /// The branch the run's worktree was created on.
    pub branch: Option<String>,
    /// URL of the pull request the run opened, if any.
    pub pr_url: Option<String>,
    pub error: Option<String>,
    pub created_at: String,
}

/// Editable fields of an automation, shared by create and update.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AutomationInput {
    pub name: String,
    pub prompt: String,
    pub agent: String,
    pub schedule: String,
    pub timezone: String,
    pub host_id: Option<i64>,
    pub project_path: Option<String>,
    /// The project's git remote URL. The command layer resolves it from
    /// the chosen project; callers may leave it `None`.
    #[serde(default)]
    pub project_remote: Option<String>,
    pub retention_limit: i64,
}

const AUTOMATION_COLUMNS: &str = "id, server_id, name, prompt, agent, schedule, \
     timezone, host_id, project_path, project_remote, enabled, retention_limit, \
     last_run_at, next_run_at, created_at, updated_at, deleted_at, dirty";

const AUTOMATION_RUN_COLUMNS: &str = "id, automation_id, status, scheduled_for, \
     started_at, finished_at, host_id, workspace_id, branch, pr_url, error, \
     created_at";

impl DatabaseStore {
    /// Insert a new automation. Marked `dirty` so a future sync pushes it.
    pub fn insert_automation(&self, input: &AutomationInput) -> Result<AutomationRecord, String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO automations
                (user_id, name, prompt, agent, schedule, timezone,
                 host_id, project_path, project_remote, retention_limit, dirty)
             VALUES ('local', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1)",
            params![
                input.name,
                input.prompt,
                input.agent,
                input.schedule,
                input.timezone,
                input.host_id,
                input.project_path,
                input.project_remote,
                input.retention_limit,
            ],
        )
        .map_err(|e| format!("Failed to insert automation: {e}"))?;
        let id = conn.last_insert_rowid();
        conn.query_row(
            &format!("SELECT {AUTOMATION_COLUMNS} FROM automations WHERE id = ?1"),
            params![id],
            row_to_automation,
        )
        .map_err(|e| format!("Failed to re-read inserted automation: {e}"))
    }

    /// All non-deleted automations for the local user, name-sorted.
    pub fn list_automations(&self) -> Vec<AutomationRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(&format!(
            "SELECT {AUTOMATION_COLUMNS} FROM automations
             WHERE user_id = 'local' AND deleted_at IS NULL
             ORDER BY name COLLATE NOCASE ASC"
        )) {
            Ok(s) => s,
            Err(error) => {
                eprintln!("[codemux::database] list_automations prepare failed: {error}");
                return Vec::new();
            }
        };
        let rows = match stmt.query_map([], row_to_automation) {
            Ok(r) => r,
            Err(error) => {
                eprintln!("[codemux::database] list_automations query_map failed: {error}");
                return Vec::new();
            }
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    /// Fetch one automation by id. `None` if it does not exist or is
    /// soft-deleted.
    pub fn get_automation(&self, id: i64) -> Option<AutomationRecord> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            &format!(
                "SELECT {AUTOMATION_COLUMNS} FROM automations
                 WHERE id = ?1 AND user_id = 'local' AND deleted_at IS NULL"
            ),
            params![id],
            row_to_automation,
        )
        .optional()
        .unwrap_or(None)
    }

    /// Update the editable fields of an automation. Marks it `dirty`.
    pub fn update_automation(
        &self,
        id: i64,
        input: &AutomationInput,
    ) -> Result<AutomationRecord, String> {
        let conn = self.conn.lock().unwrap();
        let affected = conn
            .execute(
                "UPDATE automations
                 SET name = ?1, prompt = ?2, agent = ?3, schedule = ?4,
                     timezone = ?5, host_id = ?6, project_path = ?7,
                     project_remote = ?8, retention_limit = ?9,
                     updated_at = datetime('now'), dirty = 1
                 WHERE id = ?10 AND user_id = 'local' AND deleted_at IS NULL",
                params![
                    input.name,
                    input.prompt,
                    input.agent,
                    input.schedule,
                    input.timezone,
                    input.host_id,
                    input.project_path,
                    input.project_remote,
                    input.retention_limit,
                    id,
                ],
            )
            .map_err(|e| format!("Failed to update automation: {e}"))?;
        if affected == 0 {
            return Err(format!("No automation with id {id}"));
        }
        conn.query_row(
            &format!("SELECT {AUTOMATION_COLUMNS} FROM automations WHERE id = ?1"),
            params![id],
            row_to_automation,
        )
        .map_err(|e| format!("Failed to re-read updated automation: {e}"))
    }

    /// Pause or resume an automation.
    pub fn set_automation_enabled(
        &self,
        id: i64,
        enabled: bool,
    ) -> Result<AutomationRecord, String> {
        let conn = self.conn.lock().unwrap();
        let affected = conn
            .execute(
                "UPDATE automations
                 SET enabled = ?1, updated_at = datetime('now'), dirty = 1
                 WHERE id = ?2 AND user_id = 'local' AND deleted_at IS NULL",
                params![enabled as i64, id],
            )
            .map_err(|e| format!("Failed to set automation enabled: {e}"))?;
        if affected == 0 {
            return Err(format!("No automation with id {id}"));
        }
        conn.query_row(
            &format!("SELECT {AUTOMATION_COLUMNS} FROM automations WHERE id = ?1"),
            params![id],
            row_to_automation,
        )
        .map_err(|e| format!("Failed to re-read automation: {e}"))
    }

    /// Record the computed next fire time. Scheduler bookkeeping — does
    /// not mark the row `dirty` or bump `updated_at`, since it is
    /// derived state, not a user edit.
    pub fn set_automation_next_run(
        &self,
        id: i64,
        next_run_at: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE automations SET next_run_at = ?1 WHERE id = ?2",
            params![next_run_at, id],
        )
        .map_err(|e| format!("Failed to set automation next_run_at: {e}"))?;
        Ok(())
    }

    /// Soft-delete an automation: stamp `deleted_at` and mark `dirty` so
    /// the tombstone syncs. Run-history rows are left intact.
    pub fn delete_automation(&self, id: i64) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let affected = conn
            .execute(
                "UPDATE automations
                 SET deleted_at = datetime('now'),
                     updated_at = datetime('now'), dirty = 1
                 WHERE id = ?1 AND user_id = 'local' AND deleted_at IS NULL",
                params![id],
            )
            .map_err(|e| format!("Failed to soft-delete automation: {e}"))?;
        if affected == 0 {
            return Err(format!("No automation with id {id}"));
        }
        Ok(())
    }

    /// Insert a run row. `scheduled_for` must already be floored to the
    /// minute by the caller; the `UNIQUE(automation_id, scheduled_for)`
    /// constraint makes a re-delivered fire idempotent — returns
    /// `Ok(None)` when a row for that minute already exists.
    pub fn record_automation_run(
        &self,
        automation_id: i64,
        status: &str,
        scheduled_for: &str,
        host_id: Option<i64>,
        workspace_id: Option<&str>,
    ) -> Result<Option<AutomationRunRecord>, String> {
        let conn = self.conn.lock().unwrap();
        let affected = conn
            .execute(
                "INSERT INTO automation_runs
                    (automation_id, status, scheduled_for, host_id, workspace_id)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(automation_id, scheduled_for) DO NOTHING",
                params![automation_id, status, scheduled_for, host_id, workspace_id],
            )
            .map_err(|e| format!("Failed to record automation run: {e}"))?;
        if affected == 0 {
            return Ok(None);
        }
        let id = conn.last_insert_rowid();
        conn.query_row(
            &format!("SELECT {AUTOMATION_RUN_COLUMNS} FROM automation_runs WHERE id = ?1"),
            params![id],
            row_to_automation_run,
        )
        .map(Some)
        .map_err(|e| format!("Failed to re-read automation run: {e}"))
    }

    /// Mark a run as started (the agent session is now live).
    pub fn mark_automation_run_started(&self, run_id: i64) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE automation_runs
             SET status = 'running', started_at = datetime('now')
             WHERE id = ?1",
            params![run_id],
        )
        .map_err(|e| format!("Failed to mark automation run started: {e}"))?;
        Ok(())
    }

    /// Move a run to a terminal state. `workspace_id` is written only
    /// when `Some`, so a late workspace id does not clobber an earlier
    /// one.
    pub fn finish_automation_run(
        &self,
        run_id: i64,
        status: &str,
        workspace_id: Option<&str>,
        branch: Option<&str>,
        pr_url: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        // `COALESCE` on workspace_id / branch / pr_url means a `None`
        // never clobbers a value an earlier write already set.
        conn.execute(
            "UPDATE automation_runs
             SET status = ?1,
                 finished_at = datetime('now'),
                 workspace_id = COALESCE(?2, workspace_id),
                 branch = COALESCE(?3, branch),
                 pr_url = COALESCE(?4, pr_url),
                 error = ?5
             WHERE id = ?6",
            params![status, workspace_id, branch, pr_url, error, run_id],
        )
        .map_err(|e| format!("Failed to finish automation run: {e}"))?;
        Ok(())
    }

    /// Recent runs for an automation, newest fire first.
    pub fn list_automation_runs(&self, automation_id: i64, limit: u32) -> Vec<AutomationRunRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(&format!(
            "SELECT {AUTOMATION_RUN_COLUMNS} FROM automation_runs
             WHERE automation_id = ?1
             ORDER BY scheduled_for DESC LIMIT ?2"
        )) {
            Ok(s) => s,
            Err(error) => {
                eprintln!("[codemux::database] list_automation_runs prepare failed: {error}");
                return Vec::new();
            }
        };
        let rows = match stmt.query_map(params![automation_id, limit], row_to_automation_run) {
            Ok(r) => r,
            Err(error) => {
                eprintln!("[codemux::database] list_automation_runs query_map failed: {error}");
                return Vec::new();
            }
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    /// Fail runs left in a non-terminal state by a crash or quit.
    ///
    /// A run is stale when it is still `scheduled` or `running` and its
    /// `started_at` (or `created_at`, for a run that never started) is
    /// older than `older_than`. Returns the number of rows reconciled.
    ///
    /// Both sides are wrapped in `datetime()`: `started_at` / `created_at`
    /// are stored in SQLite's `'YYYY-MM-DD HH:MM:SS'` form while callers
    /// pass an RFC 3339 ceiling — a raw string `<` would compare the
    /// space against the `T` and mis-judge same-day runs. `datetime()`
    /// normalises both formats before comparing.
    pub fn reconcile_stale_runs(&self, older_than: &str) -> usize {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE automation_runs
             SET status = 'failed',
                 finished_at = datetime('now'),
                 error = 'Run did not complete — process or app exited'
             WHERE status IN ('scheduled', 'running')
               AND datetime(COALESCE(started_at, created_at)) < datetime(?1)",
            params![older_than],
        )
        .unwrap_or(0)
    }

    // ── Automation account-sync helpers ──
    //
    // These mirror the `hosts` sync surface so `automations_sync` can
    // be a near-copy of `hosts_sync`. Only the `automations` registry
    // syncs; `automation_runs` are per-device history.

    /// Every automation row for the local user, including soft-deleted
    /// tombstones — the sync layer needs the tombstones to push.
    pub fn list_automations_for_sync(&self) -> Vec<AutomationRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(&format!(
            "SELECT {AUTOMATION_COLUMNS} FROM automations WHERE user_id = 'local'"
        )) {
            Ok(s) => s,
            Err(error) => {
                eprintln!("[codemux::database] list_automations_for_sync prepare failed: {error}");
                return Vec::new();
            }
        };
        let rows = match stmt.query_map([], row_to_automation) {
            Ok(r) => r,
            Err(error) => {
                eprintln!("[codemux::database] list_automations_for_sync query failed: {error}");
                return Vec::new();
            }
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    /// Automation rows with unpushed changes (`dirty = 1`).
    pub fn list_dirty_automations(&self) -> Vec<AutomationRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(&format!(
            "SELECT {AUTOMATION_COLUMNS} FROM automations
             WHERE user_id = 'local' AND dirty = 1"
        )) {
            Ok(s) => s,
            Err(error) => {
                eprintln!("[codemux::database] list_dirty_automations prepare failed: {error}");
                return Vec::new();
            }
        };
        let rows = match stmt.query_map([], row_to_automation) {
            Ok(r) => r,
            Err(error) => {
                eprintln!("[codemux::database] list_dirty_automations query failed: {error}");
                return Vec::new();
            }
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    /// Clear `dirty` after a successful push; optionally stamp the
    /// server-assigned id on the first upload.
    pub fn mark_automation_synced(&self, id: i64, server_id: Option<&str>) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        if let Some(sid) = server_id {
            conn.execute(
                "UPDATE automations SET dirty = 0, server_id = ?1 WHERE id = ?2",
                params![sid, id],
            )
        } else {
            conn.execute(
                "UPDATE automations SET dirty = 0 WHERE id = ?1",
                params![id],
            )
        }
        .map_err(|e| format!("Failed to mark automation synced: {e}"))?;
        Ok(())
    }

    /// Upsert a row received from the server, matched by `server_id`.
    /// Always written `dirty = 0` (server rows are authoritative).
    /// `next_run_at` / `last_run_at` are derived per-device and are not
    /// carried over the wire, so a server upsert leaves them untouched.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_automation_from_server(
        &self,
        server_id: &str,
        name: &str,
        prompt: &str,
        agent: &str,
        schedule: &str,
        timezone: &str,
        host_id: Option<i64>,
        project_path: Option<&str>,
        project_remote: Option<&str>,
        enabled: bool,
        retention_limit: i64,
        created_at: &str,
        updated_at: &str,
        deleted_at: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        let updated = conn
            .execute(
                "UPDATE automations
                 SET name = ?1, prompt = ?2, agent = ?3, schedule = ?4,
                     timezone = ?5, host_id = ?6, project_path = ?7,
                     project_remote = ?8, enabled = ?9, retention_limit = ?10,
                     created_at = ?11, updated_at = ?12, deleted_at = ?13,
                     dirty = 0
                 WHERE user_id = 'local' AND server_id = ?14",
                params![
                    name,
                    prompt,
                    agent,
                    schedule,
                    timezone,
                    host_id,
                    project_path,
                    project_remote,
                    enabled as i64,
                    retention_limit,
                    created_at,
                    updated_at,
                    deleted_at,
                    server_id,
                ],
            )
            .map_err(|e| format!("Failed to update automation from server: {e}"))?;
        if updated == 0 {
            conn.execute(
                "INSERT INTO automations
                    (user_id, server_id, name, prompt, agent, schedule,
                     timezone, host_id, project_path, project_remote,
                     enabled, retention_limit, created_at, updated_at,
                     deleted_at, dirty)
                 VALUES ('local', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                         ?11, ?12, ?13, ?14, 0)",
                params![
                    server_id,
                    name,
                    prompt,
                    agent,
                    schedule,
                    timezone,
                    host_id,
                    project_path,
                    project_remote,
                    enabled as i64,
                    retention_limit,
                    created_at,
                    updated_at,
                    deleted_at,
                ],
            )
            .map_err(|e| format!("Failed to insert automation from server: {e}"))?;
        }
        Ok(())
    }

    /// Hard-delete tombstones the server has confirmed removed. The
    /// `automation_runs` FK cascade clears their history rows too.
    pub fn purge_acknowledged_automation_deletes(&self) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM automations WHERE deleted_at IS NOT NULL AND dirty = 0",
            [],
        )
        .map_err(|e| format!("Failed to purge automation tombstones: {e}"))?;
        Ok(())
    }
}

fn row_to_automation(row: &rusqlite::Row<'_>) -> rusqlite::Result<AutomationRecord> {
    let enabled_int: i64 = row.get(10)?;
    let dirty_int: i64 = row.get(17)?;
    Ok(AutomationRecord {
        id: row.get(0)?,
        server_id: row.get(1)?,
        name: row.get(2)?,
        prompt: row.get(3)?,
        agent: row.get(4)?,
        schedule: row.get(5)?,
        timezone: row.get(6)?,
        host_id: row.get(7)?,
        project_path: row.get(8)?,
        project_remote: row.get(9)?,
        enabled: enabled_int != 0,
        retention_limit: row.get(11)?,
        last_run_at: row.get(12)?,
        next_run_at: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
        deleted_at: row.get(16)?,
        dirty: dirty_int != 0,
    })
}

fn row_to_automation_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<AutomationRunRecord> {
    Ok(AutomationRunRecord {
        id: row.get(0)?,
        automation_id: row.get(1)?,
        status: row.get(2)?,
        scheduled_for: row.get(3)?,
        started_at: row.get(4)?,
        finished_at: row.get(5)?,
        host_id: row.get(6)?,
        workspace_id: row.get(7)?,
        branch: row.get(8)?,
        pr_url: row.get(9)?,
        error: row.get(10)?,
        created_at: row.get(11)?,
    })
}

// ── Agent Usage Ledger ──
//
// Append-only accounting behind Settings → Usage. See the
// `agent_usage_ledger` DDL in `create_schema` for why it has no foreign
// key to `agent_chat_sessions`.

/// One row of the usage ledger, as read back for aggregation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageLedgerRow {
    /// Unix milliseconds.
    pub created_at: i64,
    pub thread_id: String,
    pub workspace_id: Option<String>,
    pub provider: String,
    pub model: Option<String>,
    pub subagent: bool,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    /// Informational subset of `output_tokens` — excluded from
    /// [`total_tokens`](Self::total_tokens) by design.
    #[serde(default)]
    pub reasoning_tokens: i64,
    pub cost_usd: Option<f64>,
    /// `"provider"` | `"table"` | `None`.
    #[serde(default)]
    pub cost_source: Option<String>,
    /// `"provider_history"` for rebuildable imported rows, or `"live"` for
    /// exact provider bills that have no separate local-history importer.
    #[serde(default)]
    pub source: String,
}

/// One normalized provider-history record ready for the materialized cache.
#[derive(Debug, Clone)]
pub struct ProviderUsageCacheRow {
    pub import_key: String,
    pub created_at: i64,
    pub thread_id: String,
    pub provider: String,
    pub model: Option<String>,
    pub subagent: bool,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
    pub cost_usd: Option<f64>,
    pub cost_source: Option<String>,
}

const UPSERT_PROVIDER_USAGE_SQL: &str =
    "INSERT INTO agent_usage_ledger (
         created_at, thread_id, workspace_id, provider, model, subagent,
         input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
         reasoning_tokens, cost_usd, cost_source, source, import_key
     ) VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'provider_history', ?13)
     ON CONFLICT(import_key) WHERE import_key IS NOT NULL DO UPDATE SET
         created_at = excluded.created_at,
         thread_id = excluded.thread_id,
         provider = excluded.provider,
         model = COALESCE(excluded.model, agent_usage_ledger.model),
         subagent = MAX(agent_usage_ledger.subagent, excluded.subagent),
         input_tokens = MAX(agent_usage_ledger.input_tokens, excluded.input_tokens),
         output_tokens = MAX(agent_usage_ledger.output_tokens, excluded.output_tokens),
         cache_read_tokens = MAX(agent_usage_ledger.cache_read_tokens, excluded.cache_read_tokens),
         cache_write_tokens = MAX(agent_usage_ledger.cache_write_tokens, excluded.cache_write_tokens),
         reasoning_tokens = MAX(agent_usage_ledger.reasoning_tokens, excluded.reasoning_tokens),
         cost_usd = CASE
             WHEN excluded.cost_source = 'provider' THEN excluded.cost_usd
             WHEN agent_usage_ledger.cost_source = 'provider' THEN agent_usage_ledger.cost_usd
             WHEN agent_usage_ledger.cost_usd IS NULL THEN excluded.cost_usd
             WHEN excluded.cost_usd IS NULL THEN agent_usage_ledger.cost_usd
             ELSE MAX(agent_usage_ledger.cost_usd, excluded.cost_usd)
         END,
         cost_source = CASE
             WHEN excluded.cost_source = 'provider' THEN excluded.cost_source
             WHEN agent_usage_ledger.cost_source = 'provider' THEN agent_usage_ledger.cost_source
             ELSE COALESCE(excluded.cost_source, agent_usage_ledger.cost_source)
         END,
         source = excluded.source
     WHERE agent_usage_ledger.created_at IS NOT excluded.created_at
        OR agent_usage_ledger.thread_id IS NOT excluded.thread_id
        OR agent_usage_ledger.provider IS NOT excluded.provider
        OR agent_usage_ledger.model IS NOT excluded.model
        OR agent_usage_ledger.subagent IS NOT excluded.subagent
        OR agent_usage_ledger.input_tokens IS NOT excluded.input_tokens
        OR agent_usage_ledger.output_tokens IS NOT excluded.output_tokens
        OR agent_usage_ledger.cache_read_tokens IS NOT excluded.cache_read_tokens
        OR agent_usage_ledger.cache_write_tokens IS NOT excluded.cache_write_tokens
        OR agent_usage_ledger.reasoning_tokens IS NOT excluded.reasoning_tokens
        OR agent_usage_ledger.cost_usd IS NOT excluded.cost_usd
        OR agent_usage_ledger.cost_source IS NOT excluded.cost_source
        OR agent_usage_ledger.source IS NOT excluded.source";

impl UsageLedgerRow {
    /// All four token buckets summed. Sound because the buckets are
    /// non-overlapping by construction in the provider-history importers.
    ///
    /// `reasoning_tokens` is deliberately NOT added: it is a subset of
    /// `output_tokens` and counting it would double-bill.
    pub fn total_tokens(&self) -> i64 {
        self.input_tokens + self.output_tokens + self.cache_read_tokens + self.cache_write_tokens
    }

    /// Observed input — every token that entered the prompt, cached or
    /// not. The denominator for "cached input is N% of input".
    pub fn observed_input(&self) -> i64 {
        self.input_tokens + self.cache_read_tokens + self.cache_write_tokens
    }
}

impl DatabaseStore {
    /// Insert a synthetic usage row for aggregation tests.
    ///
    /// `created_at` is supplied by the caller rather than defaulted in
    /// SQL so tests can write rows at chosen timestamps and exercise the
    /// bucketing without waiting for wall-clock time to pass.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn insert_usage_row(
        &self,
        created_at: i64,
        thread_id: &str,
        workspace_id: Option<&str>,
        provider: &str,
        model: Option<&str>,
        subagent: bool,
        input_tokens: i64,
        output_tokens: i64,
        cache_read_tokens: i64,
        cache_write_tokens: i64,
        reasoning_tokens: i64,
        cost_usd: Option<f64>,
        cost_source: Option<&str>,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO agent_usage_ledger (
                 created_at, thread_id, workspace_id, provider, model, subagent,
                 input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                 reasoning_tokens, cost_usd, cost_source
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                created_at,
                thread_id,
                workspace_id,
                provider,
                model,
                subagent as i64,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_write_tokens,
                reasoning_tokens,
                cost_usd,
                cost_source,
            ],
        )
        .map_err(|e| format!("Failed to insert usage row: {e}"))?;
        Ok(())
    }

    /// Token totals for a synthetic test thread + provider.
    ///
    /// Retained only to verify that reasoning remains a subset of output and
    /// never enters the four-way total. Returns
    /// `(input, output, cache_read, cache_write, reasoning)`.
    #[cfg(test)]
    pub fn recorded_usage_totals(
        &self,
        thread_id: &str,
        provider: &str,
    ) -> Result<(i64, i64, i64, i64, i64), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(cache_read_tokens), 0), COALESCE(SUM(cache_write_tokens), 0),
                    COALESCE(SUM(reasoning_tokens), 0)
             FROM agent_usage_ledger
             WHERE thread_id = ?1 AND provider = ?2 AND subagent = 0",
            params![thread_id, provider],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .map_err(|e| format!("Failed to sum usage rows: {e}"))
    }

    /// Materialize one provider-history record.
    ///
    /// A provider can append several snapshots for the same response while
    /// it is active. The stable `import_key` therefore upserts rather than
    /// ignoring a row forever: unchanged records are no-ops, while a growing
    /// response replaces its earlier partial snapshot. Token counters are
    /// high-water marks so an older overlapping OpenCode store cannot shrink
    /// a newer SQLite record.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_provider_usage_row(
        &self,
        import_key: &str,
        created_at: i64,
        thread_id: &str,
        provider: &str,
        model: Option<&str>,
        subagent: bool,
        input_tokens: i64,
        output_tokens: i64,
        cache_read_tokens: i64,
        cache_write_tokens: i64,
        reasoning_tokens: i64,
        cost_usd: Option<f64>,
        cost_source: Option<&str>,
    ) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let changed = conn
            .execute(
                "INSERT INTO agent_usage_ledger (
                     created_at, thread_id, workspace_id, provider, model, subagent,
                     input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, cost_usd, cost_source, source, import_key
                 ) VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'provider_history', ?13)
                 ON CONFLICT(import_key) WHERE import_key IS NOT NULL DO UPDATE SET
                     created_at = excluded.created_at,
                     thread_id = excluded.thread_id,
                     provider = excluded.provider,
                     model = COALESCE(excluded.model, agent_usage_ledger.model),
                     subagent = MAX(agent_usage_ledger.subagent, excluded.subagent),
                     input_tokens = MAX(agent_usage_ledger.input_tokens, excluded.input_tokens),
                     output_tokens = MAX(agent_usage_ledger.output_tokens, excluded.output_tokens),
                     cache_read_tokens = MAX(agent_usage_ledger.cache_read_tokens, excluded.cache_read_tokens),
                     cache_write_tokens = MAX(agent_usage_ledger.cache_write_tokens, excluded.cache_write_tokens),
                     reasoning_tokens = MAX(agent_usage_ledger.reasoning_tokens, excluded.reasoning_tokens),
                     cost_usd = CASE
                         WHEN excluded.cost_source = 'provider' THEN excluded.cost_usd
                         WHEN agent_usage_ledger.cost_source = 'provider' THEN agent_usage_ledger.cost_usd
                         WHEN agent_usage_ledger.cost_usd IS NULL THEN excluded.cost_usd
                         WHEN excluded.cost_usd IS NULL THEN agent_usage_ledger.cost_usd
                         ELSE MAX(agent_usage_ledger.cost_usd, excluded.cost_usd)
                     END,
                     cost_source = CASE
                         WHEN excluded.cost_source = 'provider' THEN excluded.cost_source
                         WHEN agent_usage_ledger.cost_source = 'provider' THEN agent_usage_ledger.cost_source
                         ELSE COALESCE(excluded.cost_source, agent_usage_ledger.cost_source)
                     END,
                     source = excluded.source
                 WHERE agent_usage_ledger.created_at IS NOT excluded.created_at
                    OR agent_usage_ledger.thread_id IS NOT excluded.thread_id
                    OR agent_usage_ledger.provider IS NOT excluded.provider
                    OR agent_usage_ledger.model IS NOT excluded.model
                    OR agent_usage_ledger.subagent IS NOT excluded.subagent
                    OR agent_usage_ledger.input_tokens IS NOT excluded.input_tokens
                    OR agent_usage_ledger.output_tokens IS NOT excluded.output_tokens
                    OR agent_usage_ledger.cache_read_tokens IS NOT excluded.cache_read_tokens
                    OR agent_usage_ledger.cache_write_tokens IS NOT excluded.cache_write_tokens
                    OR agent_usage_ledger.reasoning_tokens IS NOT excluded.reasoning_tokens
                    OR agent_usage_ledger.cost_usd IS NOT excluded.cost_usd
                    OR agent_usage_ledger.cost_source IS NOT excluded.cost_source
                    OR agent_usage_ledger.source IS NOT excluded.source",
                params![
                    created_at,
                    thread_id,
                    provider,
                    model,
                    subagent as i64,
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_write_tokens,
                    reasoning_tokens,
                    cost_usd,
                    cost_source,
                    import_key,
                ],
            )
            .map_err(|e| format!("Failed to upsert provider usage row: {e}"))?;
        Ok(changed > 0)
    }

    /// Persist one exact Grok ACP turn bill.
    ///
    /// Grok does not expose a separate local history source for the usage
    /// importer, so its authoritative `PromptResponse._meta.usage` must enter
    /// the ledger at runtime. `import_key` is derived from the durable Codemux
    /// thread id plus its unique turn id; replaying the same terminal event is
    /// therefore an upsert, never a second charge. No table-price fallback is
    /// performed here: an absent provider cost remains unknown.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_grok_live_usage_row(
        &self,
        import_key: &str,
        created_at: i64,
        thread_id: &str,
        model: Option<&str>,
        subagent: bool,
        input_tokens: i64,
        output_tokens: i64,
        cache_read_tokens: i64,
        cache_write_tokens: i64,
        reasoning_tokens: i64,
        cost_usd: Option<f64>,
        cost_source: Option<&str>,
    ) -> Result<bool, String> {
        // A cost label without a cost would claim precision the provider did
        // not supply. Keep the two nullable fields coupled at the sink too.
        let cost_source = cost_usd.and(cost_source);
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let changed = conn
            .execute(
                "INSERT INTO agent_usage_ledger (
                     created_at, thread_id, workspace_id, provider, model, subagent,
                     input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                     reasoning_tokens, cost_usd, cost_source, source, import_key
                 ) VALUES (
                     ?1, ?2,
                     (SELECT workspace_id FROM agent_chat_sessions WHERE thread_id = ?2),
                     'grok', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'live', ?12
                 )
                 ON CONFLICT(import_key) WHERE import_key IS NOT NULL DO UPDATE SET
                     created_at = MIN(agent_usage_ledger.created_at, excluded.created_at),
                     workspace_id = COALESCE(agent_usage_ledger.workspace_id, excluded.workspace_id),
                     model = COALESCE(excluded.model, agent_usage_ledger.model),
                     subagent = MAX(agent_usage_ledger.subagent, excluded.subagent),
                     input_tokens = MAX(agent_usage_ledger.input_tokens, excluded.input_tokens),
                     output_tokens = MAX(agent_usage_ledger.output_tokens, excluded.output_tokens),
                     cache_read_tokens = MAX(agent_usage_ledger.cache_read_tokens, excluded.cache_read_tokens),
                     cache_write_tokens = MAX(agent_usage_ledger.cache_write_tokens, excluded.cache_write_tokens),
                     reasoning_tokens = MAX(agent_usage_ledger.reasoning_tokens, excluded.reasoning_tokens),
                     cost_usd = CASE
                         WHEN excluded.cost_source = 'provider' THEN excluded.cost_usd
                         WHEN agent_usage_ledger.cost_source = 'provider' THEN agent_usage_ledger.cost_usd
                         ELSE COALESCE(excluded.cost_usd, agent_usage_ledger.cost_usd)
                     END,
                     cost_source = CASE
                         WHEN excluded.cost_source = 'provider' THEN excluded.cost_source
                         WHEN agent_usage_ledger.cost_source = 'provider' THEN agent_usage_ledger.cost_source
                         ELSE COALESCE(excluded.cost_source, agent_usage_ledger.cost_source)
                     END,
                     source = 'live'",
                params![
                    created_at,
                    thread_id,
                    model,
                    subagent as i64,
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_write_tokens,
                    reasoning_tokens,
                    cost_usd,
                    cost_source,
                    import_key,
                ],
            )
            .map_err(|e| format!("Failed to upsert Grok live usage row: {e}"))?;
        Ok(changed > 0)
    }

    /// Materialize many provider records in one transaction. The first scan
    /// can contain tens of thousands of OpenCode messages, so one fsync per
    /// row would make an otherwise read-only history scan unreasonably slow.
    pub fn upsert_provider_usage_rows(
        &self,
        rows: &[ProviderUsageCacheRow],
    ) -> Result<usize, String> {
        if rows.is_empty() {
            return Ok(0);
        }
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let mut changed = 0;
        {
            let mut stmt = tx
                .prepare_cached(UPSERT_PROVIDER_USAGE_SQL)
                .map_err(|e| format!("Failed to prepare provider usage upsert: {e}"))?;
            for row in rows {
                changed += stmt
                    .execute(params![
                        row.created_at,
                        row.thread_id,
                        row.provider,
                        row.model,
                        row.subagent as i64,
                        row.input_tokens,
                        row.output_tokens,
                        row.cache_read_tokens,
                        row.cache_write_tokens,
                        row.reasoning_tokens,
                        row.cost_usd,
                        row.cost_source,
                        row.import_key,
                    ])
                    .map_err(|e| format!("Failed to upsert provider usage row: {e}"))?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(changed)
    }

    /// How much of `path` a previous scan already consumed, if any.
    /// Returns `(mtime_ms, size_bytes, consumed_bytes)`.
    pub fn usage_import_state(&self, path: &str) -> Result<Option<(i64, i64, i64)>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT mtime_ms, size_bytes, consumed_bytes FROM usage_import_state WHERE path = ?1",
            params![path],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|e| format!("Failed to read import state: {e}"))
    }

    /// Record how far `path` has now been consumed.
    pub fn set_usage_import_state(
        &self,
        path: &str,
        mtime_ms: i64,
        size_bytes: i64,
        consumed_bytes: i64,
        scanned_at: i64,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO usage_import_state (path, mtime_ms, size_bytes, consumed_bytes, scanned_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(path) DO UPDATE SET
                 mtime_ms = ?2, size_bytes = ?3, consumed_bytes = ?4, scanned_at = ?5",
            params![path, mtime_ms, size_bytes, consumed_bytes, scanned_at],
        )
        .map_err(|e| format!("Failed to write import state: {e}"))?;
        Ok(())
    }

    /// Record signatures for a completed provider-history batch.
    pub fn set_usage_import_states(
        &self,
        states: &[(String, i64, i64)],
        scanned_at: i64,
    ) -> Result<(), String> {
        if states.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT INTO usage_import_state
                         (path, mtime_ms, size_bytes, consumed_bytes, scanned_at)
                     VALUES (?1, ?2, ?3, ?3, ?4)
                     ON CONFLICT(path) DO UPDATE SET
                         mtime_ms = ?2, size_bytes = ?3,
                         consumed_bytes = ?3, scanned_at = ?4",
                )
                .map_err(|e| format!("Failed to prepare import-state upsert: {e}"))?;
            for (path, mtime_ms, size_bytes) in states {
                stmt.execute(params![path, mtime_ms, size_bytes, scanned_at])
                    .map_err(|e| format!("Failed to write import state: {e}"))?;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Drop the rebuildable usage cache and its source signatures.
    /// Provider history remains authoritative, so the next scan can rebuild
    /// every imported row after a parser or pricing change. Exact Grok live
    /// rows have no second history source and must survive that rebuild. Other
    /// legacy live rows are removed because their providers *do* have history
    /// importers and retaining them would double-count Codemux-launched work.
    pub fn reset_usage_history(&self) -> Result<usize, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        let removed = tx
            .execute(
                "DELETE FROM agent_usage_ledger
                 WHERE provider != 'grok' OR source != 'live'",
                [],
            )
            .map_err(|e| format!("Failed to clear usage cache: {e}"))?;
        tx.execute("DELETE FROM usage_import_state", [])
            .map_err(|e| format!("Failed to clear import state: {e}"))?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(removed)
    }

    /// Every ledger row at or after `since_ms`, oldest first.
    ///
    /// The range filter and ordering are pushed into SQL (that is what
    /// `idx_agent_usage_ledger_created` exists for); the grouping into
    /// buckets / providers / models is done in Rust. Splitting it that
    /// way keeps the aggregation a pure function over a `Vec` — the
    /// dashboard needs the same rows sliced three different ways, and
    /// three GROUP BY round-trips would have to agree with each other by
    /// hand.
    pub fn usage_rows_since(&self, since_ms: i64) -> Result<Vec<UsageLedgerRow>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT created_at, thread_id, workspace_id, provider, model, subagent,
                        input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                        reasoning_tokens, cost_usd, cost_source, source
                 FROM agent_usage_ledger
                 WHERE created_at >= ?1
                 ORDER BY created_at ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![since_ms], |row| {
                Ok(UsageLedgerRow {
                    created_at: row.get(0)?,
                    thread_id: row.get(1)?,
                    workspace_id: row.get(2)?,
                    provider: row.get(3)?,
                    model: row.get(4)?,
                    subagent: row.get::<_, i64>(5)? != 0,
                    input_tokens: row.get(6)?,
                    output_tokens: row.get(7)?,
                    cache_read_tokens: row.get(8)?,
                    cache_write_tokens: row.get(9)?,
                    reasoning_tokens: row.get(10)?,
                    cost_usd: row.get(11)?,
                    cost_source: row.get(12)?,
                    source: row.get(13)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        Ok(rows)
    }
}

// ── Agent Chat Sessions ──
//
// Persistence for the chat-history dropdown. One row per thread the
// user has ever opened in a given workspace/cwd. Rows are cheap
// (text-only, ~200 bytes each) so we never prune automatically — the
// user deletes explicitly from the dropdown.

impl DatabaseStore {
    /// Insert or refresh a session row. Called from
    /// `agent_chat_start_session`: on a brand-new chat we INSERT, on a
    /// silent restart (same thread_id after migrate) the ON CONFLICT
    /// bumps `last_active_at` and preserves `sdk_session_id`/`title`.
    /// A provider handoff keeps the human-facing title but atomically
    /// clears `sdk_session_id`: provider-native resume cursors cannot be
    /// passed between adapters.
    pub fn upsert_agent_chat_session(
        &self,
        thread_id: &str,
        workspace_id: &str,
        cwd: Option<&str>,
        provider: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO agent_chat_sessions
                 (thread_id, workspace_id, cwd, provider, created_at, last_active_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'), datetime('now'))
             ON CONFLICT(thread_id) DO UPDATE SET
                 workspace_id = ?2,
                 cwd = ?3,
                 sdk_session_id = CASE
                     WHEN agent_chat_sessions.provider != ?4 THEN NULL
                     ELSE agent_chat_sessions.sdk_session_id
                 END,
                 provider = ?4,
                 last_active_at = datetime('now')",
            params![thread_id, workspace_id, cwd, provider],
        )
        .map_err(|e| format!("Failed to upsert agent_chat_session: {e}"))?;
        Ok(())
    }

    /// Copy metadata (title, sdk_session_id, created_at) from an old
    /// thread id to a new one. Used on silent restart so the session
    /// keeps its history-dropdown identity after
    /// `migrateThreadId`. The caller is expected to `upsert` the new
    /// row first; this just carries forward the human-facing fields.
    pub fn migrate_agent_chat_session(
        &self,
        old_thread_id: &str,
        new_thread_id: &str,
    ) -> Result<(), String> {
        if old_thread_id == new_thread_id {
            return Ok(());
        }
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agent_chat_sessions SET
                 title = COALESCE(
                     (SELECT title FROM agent_chat_sessions WHERE thread_id = ?1),
                     title
                 ),
                 sdk_session_id = COALESCE(
                     (SELECT sdk_session_id FROM agent_chat_sessions WHERE thread_id = ?1),
                     sdk_session_id
                 ),
                 created_at = COALESCE(
                     (SELECT created_at FROM agent_chat_sessions WHERE thread_id = ?1),
                     created_at
                 ),
                 model = COALESCE(
                     (SELECT model FROM agent_chat_sessions WHERE thread_id = ?1),
                     model
                 ),
                 effort = COALESCE(
                     (SELECT effort FROM agent_chat_sessions WHERE thread_id = ?1),
                     effort
                 ),
                 context_window = COALESCE(
                     (SELECT context_window FROM agent_chat_sessions WHERE thread_id = ?1),
                     context_window
                 ),
                 permission_mode = COALESCE(
                     (SELECT permission_mode FROM agent_chat_sessions WHERE thread_id = ?1),
                     permission_mode
                 ),
                 fast_mode = COALESCE(
                     (SELECT fast_mode FROM agent_chat_sessions WHERE thread_id = ?1),
                     fast_mode
                 )
             WHERE thread_id = ?2",
            params![old_thread_id, new_thread_id],
        )
        .map_err(|e| format!("Failed to migrate agent_chat_session: {e}"))?;
        // Move persisted messages over BEFORE dropping the source row,
        // otherwise ON DELETE CASCADE would wipe the transcript history
        // we just promised to carry forward.
        conn.execute(
            "UPDATE agent_chat_messages SET thread_id = ?2 WHERE thread_id = ?1",
            params![old_thread_id, new_thread_id],
        )
        .map_err(|e| format!("Failed to migrate agent_chat_messages: {e}"))?;
        conn.execute(
            "DELETE FROM agent_chat_sessions WHERE thread_id = ?1",
            params![old_thread_id],
        )
        .map_err(|e| format!("Failed to drop migrated source row: {e}"))?;
        Ok(())
    }

    /// Record the Claude Agent SDK session UUID once the sidecar's
    /// `sdk-session-id` notification has resolved. This is the value
    /// that later feeds `StartSessionInput::resume_cursor` when the
    /// user reopens the chat from the history dropdown.
    pub fn set_agent_chat_sdk_session_id(
        &self,
        thread_id: &str,
        sdk_session_id: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agent_chat_sessions
                 SET sdk_session_id = ?2, last_active_at = datetime('now')
                 WHERE thread_id = ?1",
            params![thread_id, sdk_session_id],
        )
        .map_err(|e| format!("Failed to set sdk_session_id: {e}"))?;
        Ok(())
    }

    /// Clear the persisted SDK session UUID for a thread. Called when the
    /// sidecar reports it could not resume the stored session (its on-disk
    /// conversation JSONL was gone) and rebuilt a fresh query — the dead
    /// id must never be handed back as a `resume` cursor again. The row
    /// itself survives (the visible transcript still hydrates from it); it
    /// just drops out of the history dropdown until a new `sdk-session-id`
    /// repopulates the column.
    pub fn clear_agent_chat_sdk_session_id(&self, thread_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agent_chat_sessions
                 SET sdk_session_id = NULL
                 WHERE thread_id = ?1",
            params![thread_id],
        )
        .map_err(|e| format!("Failed to clear sdk_session_id: {e}"))?;
        Ok(())
    }

    /// Overwrite the per-thread chat configuration, honouring the
    /// tri-state of each [`AgentChatSessionConfig`] field:
    ///
    /// - `None` (outer) — the column is omitted from the UPDATE (left
    ///   untouched).
    /// - `Some(None)` — the column is set to `NULL` (explicit clear).
    /// - `Some(Some(v))` — the column is set to `v`.
    ///
    /// The SET clause is built dynamically so an explicit clear can
    /// actually write `NULL`; a static `COALESCE(?, col)` could only
    /// ever leave-or-set and would silently drop a clear (that bug let a
    /// model-incompatible effort/context survive a restart). Called at
    /// session start with the `StartSessionInput` selection and again
    /// from the `agent_chat_update_session_config` command every time
    /// the user changes a picker — DB-only, no live-session requirement,
    /// so the value survives a restart even if it was never applied to a
    /// live SDK session.
    ///
    /// A no-op when no field is supplied, and when no row exists for
    /// `thread_id` (the UPDATE matches zero rows); callers persist the
    /// session row first.
    pub fn update_agent_chat_session_config(
        &self,
        thread_id: &str,
        config: &AgentChatSessionConfig,
    ) -> Result<(), String> {
        use rusqlite::types::Value;
        // Turn a tri-state field into a NULL-able bind value, or `None`
        // when the caller left the field absent (so it's skipped).
        fn bind(field: &Option<Option<String>>) -> Option<Value> {
            field.as_ref().map(|inner| match inner {
                Some(s) => Value::Text(s.clone()),
                None => Value::Null,
            })
        }
        fn bind_bool(field: Option<bool>) -> Option<Value> {
            field.map(|value| Value::Integer(i64::from(value)))
        }
        let columns = [
            ("model", bind(&config.model)),
            ("effort", bind(&config.effort)),
            ("context_window", bind(&config.context_window)),
            ("permission_mode", bind(&config.permission_mode)),
            ("fast_mode", bind_bool(config.fast_mode)),
        ];
        let mut set_clauses: Vec<String> = Vec::new();
        let mut binds: Vec<Value> = Vec::new();
        for (name, value) in columns {
            if let Some(value) = value {
                binds.push(value);
                set_clauses.push(format!("{name} = ?{}", binds.len()));
            }
        }
        if set_clauses.is_empty() {
            // Nothing to write — every field was left untouched.
            return Ok(());
        }
        binds.push(Value::Text(thread_id.to_string()));
        let sql = format!(
            "UPDATE agent_chat_sessions SET {} WHERE thread_id = ?{}",
            set_clauses.join(", "),
            binds.len(),
        );
        let conn = self.conn.lock().unwrap();
        conn.execute(&sql, rusqlite::params_from_iter(binds))
            .map_err(|e| format!("Failed to update agent_chat_session config: {e}"))?;
        Ok(())
    }

    /// Set (or replace) the dropdown title for a session. Called
    /// once from the first-turn auto-title path and again any time
    /// the user renames the session from the dropdown.
    pub fn set_agent_chat_title(&self, thread_id: &str, title: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agent_chat_sessions
                 SET title = ?2, last_active_at = datetime('now')
                 WHERE thread_id = ?1",
            params![thread_id, title],
        )
        .map_err(|e| format!("Failed to set title: {e}"))?;
        Ok(())
    }

    /// Returns the current title for a session, or None if the
    /// session is absent / has no title yet. Used by the auto-title
    /// path to check "should I generate a title from this turn".
    pub fn get_agent_chat_title(&self, thread_id: &str) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT title FROM agent_chat_sessions WHERE thread_id = ?1",
            params![thread_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
    }

    /// Bump `last_active_at` so an active session floats to the top
    /// of the dropdown. Called on every user turn.
    pub fn touch_agent_chat_session(&self, thread_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agent_chat_sessions
                 SET last_active_at = datetime('now')
                 WHERE thread_id = ?1",
            params![thread_id],
        )
        .map_err(|e| format!("Failed to touch session: {e}"))?;
        Ok(())
    }

    /// List sessions for the dropdown, ordered by recency. Scoped to
    /// a single workspace. When `cwd` is Some the list is further
    /// filtered to sessions opened from that exact directory so the
    /// dropdown matches the pane the user is looking at.
    pub fn list_agent_chat_sessions(
        &self,
        workspace_id: &str,
        cwd: Option<&str>,
        limit: u32,
    ) -> Vec<AgentChatSessionRecord> {
        let conn = self.conn.lock().unwrap();
        let map_row = |row: &rusqlite::Row<'_>| {
            Ok(AgentChatSessionRecord {
                thread_id: row.get(0)?,
                sdk_session_id: row.get(1)?,
                workspace_id: row.get(2)?,
                cwd: row.get(3)?,
                provider: row.get(4)?,
                title: row.get(5)?,
                created_at: row.get(6)?,
                last_active_at: row.get(7)?,
                model: row.get(8)?,
                effort: row.get(9)?,
                context_window: row.get(10)?,
                permission_mode: row.get(11)?,
                fast_mode: row.get::<_, Option<i64>>(12)?.unwrap_or(0) != 0,
            })
        };
        // Only surface rows that actually have an sdk_session_id —
        // without one the Claude SDK cannot resume the conversation
        // so offering them in the dropdown leads to a dead-end toast.
        // Rows are created with null sdk_session_id and get the UUID
        // once `ResumeCursorUpdated` fires (after the first SDK
        // message), so this filter cleanly hides transient rows from
        // silent restarts that never got interacted with.
        if let Some(cwd) = cwd {
            let mut stmt = match conn.prepare(
                "SELECT thread_id, sdk_session_id, workspace_id, cwd, provider, title, created_at, last_active_at, model, effort, context_window, permission_mode, fast_mode
                 FROM agent_chat_sessions
                 WHERE workspace_id = ?1 AND cwd = ?2 AND sdk_session_id IS NOT NULL
                 ORDER BY last_active_at DESC LIMIT ?3",
            ) {
                Ok(s) => s,
                Err(_) => return Vec::new(),
            };
            stmt.query_map(params![workspace_id, cwd, limit], map_row)
                .map(|iter| iter.filter_map(|r| r.ok()).collect())
                .unwrap_or_default()
        } else {
            let mut stmt = match conn.prepare(
                "SELECT thread_id, sdk_session_id, workspace_id, cwd, provider, title, created_at, last_active_at, model, effort, context_window, permission_mode, fast_mode
                 FROM agent_chat_sessions
                 WHERE workspace_id = ?1 AND sdk_session_id IS NOT NULL
                 ORDER BY last_active_at DESC LIMIT ?2",
            ) {
                Ok(s) => s,
                Err(_) => return Vec::new(),
            };
            stmt.query_map(params![workspace_id, limit], map_row)
                .map(|iter| iter.filter_map(|r| r.ok()).collect())
                .unwrap_or_default()
        }
    }

    /// List conversations that can be attached through `@session:`.
    ///
    /// The workspace boundary is enforced in SQL, the current thread is
    /// omitted to prevent self-referential prompts, and conversations in the
    /// current checkout sort ahead of other worktrees in the same workspace.
    /// A correlated lookup supplies the latest visible user/assistant prose as
    /// a compact picker preview without hydrating full event transcripts.
    pub fn list_agent_chat_session_mentions(
        &self,
        workspace_id: &str,
        current_cwd: Option<&str>,
        exclude_thread_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<AgentChatSessionMention>, String> {
        let conn = self.conn.lock().unwrap();
        let limit = limit.clamp(1, 50);
        let mut stmt = conn
            .prepare(
                "SELECT
                     s.thread_id,
                     s.workspace_id,
                     s.cwd,
                     s.provider,
                     s.title,
                     s.last_active_at,
                     COALESCE((
                         SELECT f.content
                         FROM agent_chat_search f
                         JOIN agent_chat_messages m ON m.id = f.rowid
                         WHERE f.thread_id = s.thread_id
                         ORDER BY m.id DESC
                         LIMIT 1
                     ), ''),
                     (SELECT COUNT(*)
                      FROM agent_chat_search f
                      WHERE f.thread_id = s.thread_id)
                 FROM agent_chat_sessions s
                 WHERE s.workspace_id = ?1
                   AND (?3 IS NULL OR s.thread_id <> ?3)
                   AND EXISTS (
                       SELECT 1 FROM agent_chat_search f
                       WHERE f.thread_id = s.thread_id
                   )
                 ORDER BY
                     CASE WHEN ?2 IS NOT NULL AND s.cwd = ?2 THEN 0 ELSE 1 END,
                     s.last_active_at DESC
                 LIMIT ?4",
            )
            .map_err(|e| format!("Failed to prepare session mention list: {e}"))?;
        let rows = stmt
            .query_map(
                params![workspace_id, current_cwd, exclude_thread_id, limit],
                |row| {
                    Ok(AgentChatSessionMention {
                        thread_id: row.get(0)?,
                        workspace_id: row.get(1)?,
                        cwd: row.get(2)?,
                        provider: row.get(3)?,
                        title: row.get(4)?,
                        last_active_at: row.get(5)?,
                        preview: row.get(6)?,
                        message_count: row.get::<_, i64>(7)?.max(0) as u32,
                    })
                },
            )
            .map_err(|e| format!("Failed to list session mentions: {e}"))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read session mention: {e}"))
    }

    /// Return the human-visible conversation stream for one thread. Callers
    /// must independently validate the session's workspace before using this
    /// method; the command surface does that before materialising a handoff.
    pub fn list_agent_chat_visible_messages(
        &self,
        thread_id: &str,
    ) -> Result<Vec<AgentChatVisibleMessage>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT f.role, f.content
                 FROM agent_chat_search f
                 JOIN agent_chat_messages m ON m.id = f.rowid
                 WHERE f.thread_id = ?1
                 ORDER BY m.id ASC",
            )
            .map_err(|e| format!("Failed to prepare visible session messages: {e}"))?;
        let rows = stmt
            .query_map(params![thread_id], |row| {
                Ok(AgentChatVisibleMessage {
                    role: row.get(0)?,
                    content: row.get(1)?,
                })
            })
            .map_err(|e| format!("Failed to list visible session messages: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read visible session message: {e}"))
    }

    /// Durable revision for the safe-visible transcript. The FTS row id is
    /// the backing `agent_chat_messages.id`, so it advances exactly when the
    /// summary input advances and is stable across app restarts.
    pub fn agent_chat_visible_revision(&self, thread_id: &str) -> Result<i64, String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COALESCE(MAX(rowid), 0) FROM agent_chat_search WHERE thread_id = ?1",
            params![thread_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Failed to read conversation revision: {e}"))
    }

    pub fn get_agent_chat_handoff_summary(
        &self,
        thread_id: &str,
    ) -> Result<Option<AgentChatHandoffSummaryCache>, String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT revision_message_id, summarizer_provider, summarizer_model,
                    summarizer_effort, prompt_version, summary, generated_at
             FROM agent_chat_handoff_summaries WHERE thread_id = ?1",
            params![thread_id],
            |row| {
                Ok(AgentChatHandoffSummaryCache {
                    revision_message_id: row.get(0)?,
                    summarizer_provider: row.get(1)?,
                    summarizer_model: row.get(2)?,
                    summarizer_effort: row.get(3)?,
                    prompt_version: row.get::<_, i64>(4)?.max(0) as u32,
                    summary: row.get(5)?,
                    generated_at: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(|e| format!("Failed to read handoff summary cache: {e}"))
    }

    pub fn put_agent_chat_handoff_summary(
        &self,
        thread_id: &str,
        revision_message_id: i64,
        summarizer_provider: &str,
        summarizer_model: &str,
        summarizer_effort: Option<&str>,
        prompt_version: u32,
        summary: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO agent_chat_handoff_summaries
                 (thread_id, revision_message_id, summarizer_provider,
                  summarizer_model, summarizer_effort, prompt_version, summary,
                  generated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))
             ON CONFLICT(thread_id) DO UPDATE SET
                 revision_message_id = excluded.revision_message_id,
                 summarizer_provider = excluded.summarizer_provider,
                 summarizer_model = excluded.summarizer_model,
                 summarizer_effort = excluded.summarizer_effort,
                 prompt_version = excluded.prompt_version,
                 summary = excluded.summary,
                 generated_at = datetime('now')",
            params![
                thread_id,
                revision_message_id,
                summarizer_provider,
                summarizer_model,
                summarizer_effort,
                prompt_version,
                summary,
            ],
        )
        .map_err(|e| format!("Failed to cache handoff summary: {e}"))?;
        Ok(())
    }

    /// Read the complete safe-visible conversation in bounded pages. Cursor
    /// is the last message id already seen; `None` starts at the beginning.
    pub fn read_agent_chat_history_page(
        &self,
        workspace_id: &str,
        thread_id: &str,
        after_message_id: Option<i64>,
        limit: u32,
    ) -> Result<AgentChatHistoryPage, String> {
        let record = self
            .get_agent_chat_session(thread_id)
            .ok_or_else(|| "conversation_not_found".to_string())?;
        if record.workspace_id != workspace_id {
            return Err("conversation_outside_workspace".to_string());
        }

        let limit = limit.clamp(1, 100) as usize;
        let cursor = after_message_id.unwrap_or(0).max(0);
        let conn = self.conn.lock().unwrap();
        let total_visible_messages: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_chat_search WHERE thread_id = ?1",
                params![thread_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to count visible conversation messages: {e}"))?;
        let mut stmt = conn
            .prepare(
                "SELECT f.rowid, f.role, f.content, m.created_at
                 FROM agent_chat_search f
                 JOIN agent_chat_messages m ON m.id = f.rowid
                 WHERE f.thread_id = ?1 AND f.rowid > ?2
                 ORDER BY f.rowid ASC
                 LIMIT ?3",
            )
            .map_err(|e| format!("Failed to prepare conversation page: {e}"))?;
        let rows = stmt
            .query_map(params![thread_id, cursor, (limit + 1) as i64], |row| {
                Ok(AgentChatHistoryMessage {
                    message_id: row.get(0)?,
                    role: row.get(1)?,
                    content: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })
            .map_err(|e| format!("Failed to read conversation page: {e}"))?;
        let mut messages = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to decode conversation page: {e}"))?;
        let has_more = messages.len() > limit;
        if has_more {
            messages.truncate(limit);
        }
        let next_cursor = has_more
            .then(|| messages.last().map(|message| message.message_id))
            .flatten();
        Ok(AgentChatHistoryPage {
            conversation_id: thread_id.to_string(),
            title: record.title,
            provider: record.provider,
            messages,
            next_cursor,
            total_visible_messages: total_visible_messages.max(0) as u32,
        })
    }

    pub fn search_agent_chat_history(
        &self,
        workspace_id: &str,
        thread_id: &str,
        query: &str,
        limit: u32,
    ) -> Result<Vec<AgentChatHistorySearchHit>, String> {
        let record = self
            .get_agent_chat_session(thread_id)
            .ok_or_else(|| "conversation_not_found".to_string())?;
        if record.workspace_id != workspace_id {
            return Err("conversation_outside_workspace".to_string());
        }
        let Some(fts_query) = conversation_fts_query(query) else {
            return Ok(Vec::new());
        };
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT agent_chat_search.rowid, agent_chat_search.role,
                        snippet(agent_chat_search, 0, '', '', ' … ', 24),
                        m.created_at
                 FROM agent_chat_search
                 JOIN agent_chat_messages m ON m.id = agent_chat_search.rowid
                 WHERE agent_chat_search MATCH ?1
                   AND agent_chat_search.thread_id = ?2
                 ORDER BY bm25(agent_chat_search), agent_chat_search.rowid DESC
                 LIMIT ?3",
            )
            .map_err(|e| format!("Failed to prepare conversation search: {e}"))?;
        let rows = stmt
            .query_map(params![fts_query, thread_id, limit.clamp(1, 50)], |row| {
                Ok(AgentChatHistorySearchHit {
                    message_id: row.get(0)?,
                    role: row.get(1)?,
                    snippet: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })
            .map_err(|e| format!("Failed to search conversation: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to decode conversation search: {e}"))
    }

    /// Search persisted conversation titles and human-visible transcript
    /// prose inside the supplied workspace scope. FTS prefix matching keeps
    /// partial words useful while the command palette is still being typed.
    pub fn search_agent_chat(
        &self,
        query: &str,
        workspace_ids: &[String],
        limit: u32,
    ) -> Result<Vec<AgentChatSearchResult>, String> {
        let query = query.trim();
        if query.is_empty() || workspace_ids.is_empty() {
            return Ok(Vec::new());
        }

        let limit = limit.clamp(1, 50);
        let placeholders = std::iter::repeat("?")
            .take(workspace_ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let conn = self.conn.lock().unwrap();
        let mut results = Vec::new();

        // Titles are few and short, so a case-insensitive substring scan is
        // more natural than token-prefix FTS and gives one row per session.
        let title_sql = format!(
            "SELECT thread_id, workspace_id, cwd, provider, title, last_active_at
             FROM agent_chat_sessions
             WHERE workspace_id IN ({placeholders})
               AND instr(lower(COALESCE(title, '')), lower(?)) > 0
             ORDER BY last_active_at DESC
             LIMIT {limit}"
        );
        let mut title_params: Vec<Value> = workspace_ids.iter().cloned().map(Value::Text).collect();
        title_params.push(Value::Text(query.to_string()));
        let mut title_stmt = conn
            .prepare(&title_sql)
            .map_err(|e| format!("Failed to prepare agent chat title search: {e}"))?;
        let title_rows = title_stmt
            .query_map(params_from_iter(title_params), |row| {
                let title: Option<String> = row.get(4)?;
                Ok(AgentChatSearchResult {
                    message_id: None,
                    thread_id: row.get(0)?,
                    workspace_id: row.get(1)?,
                    cwd: row.get(2)?,
                    provider: row.get(3)?,
                    session_title: title.clone(),
                    role: "title".to_string(),
                    turn_id: None,
                    snippet: title.unwrap_or_default(),
                    created_at: row.get(5)?,
                })
            })
            .map_err(|e| format!("Failed to search agent chat titles: {e}"))?;
        for row in title_rows {
            results.push(row.map_err(|e| format!("Failed to read agent chat title hit: {e}"))?);
        }

        if results.len() >= limit as usize {
            results.truncate(limit as usize);
            return Ok(results);
        }

        let Some(fts_query) = conversation_fts_query(query) else {
            return Ok(results);
        };
        let title_threads: HashSet<String> = results
            .iter()
            .map(|result| result.thread_id.clone())
            .collect();
        let content_fetch_limit = limit.saturating_mul(2);
        let content_sql = format!(
            "SELECT
                 m.id,
                 s.thread_id,
                 s.workspace_id,
                 s.cwd,
                 s.provider,
                 s.title,
                 agent_chat_search.role,
                 agent_chat_search.turn_id,
                 snippet(agent_chat_search, 0, '', '', ' … ', 18),
                 m.created_at
             FROM agent_chat_search
             JOIN agent_chat_messages m ON m.id = agent_chat_search.rowid
             JOIN agent_chat_sessions s ON s.thread_id = m.thread_id
             WHERE agent_chat_search MATCH ?
               AND s.workspace_id IN ({placeholders})
             ORDER BY bm25(agent_chat_search), m.id DESC
             LIMIT {content_fetch_limit}"
        );
        let mut content_params = vec![Value::Text(fts_query)];
        content_params.extend(workspace_ids.iter().cloned().map(Value::Text));
        let mut content_stmt = conn
            .prepare(&content_sql)
            .map_err(|e| format!("Failed to prepare agent chat content search: {e}"))?;
        let content_rows = content_stmt
            .query_map(params_from_iter(content_params), |row| {
                Ok(AgentChatSearchResult {
                    message_id: Some(row.get(0)?),
                    thread_id: row.get(1)?,
                    workspace_id: row.get(2)?,
                    cwd: row.get(3)?,
                    provider: row.get(4)?,
                    session_title: row.get(5)?,
                    role: row.get(6)?,
                    turn_id: row.get(7)?,
                    snippet: row.get(8)?,
                    created_at: row.get(9)?,
                })
            })
            .map_err(|e| format!("Failed to search agent chat content: {e}"))?;
        for row in content_rows {
            let row = row.map_err(|e| format!("Failed to read agent chat content hit: {e}"))?;
            // A title match is already a better representation of the same
            // thread, so do not spend a result slot repeating it.
            if title_threads.contains(&row.thread_id) {
                continue;
            }
            results.push(row);
            if results.len() == limit as usize {
                break;
            }
        }

        Ok(results)
    }

    /// De-duplicate sessions that share an `sdk_session_id`. When the
    /// user resumes a past chat we start a NEW provider session with
    /// a new `thread_id`, but its SDK session id matches an existing
    /// row's. Without cleanup the dropdown would eventually show
    /// duplicate entries for the same logical conversation. This
    /// collapses all rows sharing an `sdk_session_id` into the most
    /// recent one, carrying forward the best title / earliest
    /// created_at.
    pub fn collapse_duplicate_agent_chat_sessions(
        &self,
        sdk_session_id: &str,
    ) -> Result<AgentChatSessionCleanup, String> {
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn
            .transaction()
            .map_err(|e| format!("Failed to start duplicate-session merge: {e}"))?;
        // Pick the most-recently-active row as the survivor.
        let survivor: Option<String> = transaction
            .query_row(
                "SELECT thread_id FROM agent_chat_sessions
                 WHERE sdk_session_id = ?1
                 ORDER BY last_active_at DESC LIMIT 1",
                params![sdk_session_id],
                |row| row.get(0),
            )
            .ok();
        let Some(survivor) = survivor else {
            return Ok(AgentChatSessionCleanup::default());
        };

        // Capture every external resource owned by rows that the FK cascade is
        // about to remove. Git refs and chat images live outside SQLite, so the
        // caller must delete them after this transaction commits.
        let removed_sessions = {
            let mut statement = transaction
                .prepare(
                    "SELECT thread_id, cwd FROM agent_chat_sessions
                     WHERE sdk_session_id = ?1 AND thread_id != ?2",
                )
                .map_err(|e| format!("Failed to prepare duplicate-session cleanup: {e}"))?;
            let sessions = statement
                .query_map(params![sdk_session_id, survivor], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
                })
                .map_err(|e| format!("Failed to read duplicate-session cleanup: {e}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to collect duplicate-session cleanup: {e}"))?;
            sessions
        };
        let mut cleanup = AgentChatSessionCleanup {
            thread_ids: removed_sessions
                .iter()
                .map(|(thread_id, _)| thread_id.clone())
                .collect(),
            ..AgentChatSessionCleanup::default()
        };
        let mut repo_paths = removed_sessions
            .iter()
            .filter_map(|(thread_id, cwd)| {
                cwd.as_ref().map(|cwd| (thread_id.clone(), cwd.clone()))
            })
            .collect::<HashSet<_>>();
        let mut checkpoint_refs = HashSet::new();
        for (thread_id, _) in &removed_sessions {
            let mut turn_statement = transaction
                .prepare(
                    "SELECT repo_path, ref_name FROM agent_chat_turn_checkpoints
                     WHERE thread_id = ?1",
                )
                .map_err(|e| format!("Failed to prepare turn-checkpoint cleanup: {e}"))?;
            let turn_refs = turn_statement
                .query_map(params![thread_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|e| format!("Failed to read turn-checkpoint cleanup: {e}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to collect turn-checkpoint cleanup: {e}"))?;
            for (repo_path, ref_name) in turn_refs {
                repo_paths.insert((thread_id.clone(), repo_path.clone()));
                checkpoint_refs.insert((repo_path, ref_name));
            }
            let legacy_ref = transaction
                .query_row(
                    "SELECT repo_path, ref_name FROM agent_chat_checkpoints
                     WHERE thread_id = ?1",
                    params![thread_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(|e| format!("Failed to read run-checkpoint cleanup: {e}"))?;
            if let Some((repo_path, ref_name)) = legacy_ref {
                repo_paths.insert((thread_id.clone(), repo_path.clone()));
                checkpoint_refs.insert((repo_path, ref_name));
            }
        }
        cleanup.repo_paths = repo_paths.into_iter().collect();
        cleanup.checkpoint_refs = checkpoint_refs.into_iter().collect();
        cleanup.thread_ids.sort();
        cleanup.repo_paths.sort();
        cleanup.checkpoint_refs.sort();
        // Merge identity + config fields onto the survivor: keep the
        // earliest created_at, and carry forward any non-null title /
        // per-thread config (model / effort / context_window /
        // permission_mode / fast_mode) across the whole set. The survivor is the
        // most-recently-active row — for a resume that's the freshly
        // minted thread whose config columns are still NULL — so
        // without this backfill the user's persisted per-thread model /
        // effort / context / permission-mode / speed selection would be lost
        // when the original row is DELETEd below. Each config column
        // prefers the survivor's own non-null value, then the
        // most-recently-active non-null value across the group.
        transaction.execute(
            "UPDATE agent_chat_sessions
                 SET created_at = COALESCE(
                     (SELECT MIN(created_at) FROM agent_chat_sessions
                      WHERE sdk_session_id = ?1),
                     created_at
                 ),
                 title = COALESCE(
                     title,
                     (SELECT title FROM agent_chat_sessions
                      WHERE sdk_session_id = ?1 AND title IS NOT NULL
                      ORDER BY last_active_at DESC LIMIT 1)
                 ),
                 model = COALESCE(
                     model,
                     (SELECT model FROM agent_chat_sessions
                      WHERE sdk_session_id = ?1 AND model IS NOT NULL
                      ORDER BY last_active_at DESC LIMIT 1)
                 ),
                 effort = COALESCE(
                     effort,
                     (SELECT effort FROM agent_chat_sessions
                      WHERE sdk_session_id = ?1 AND effort IS NOT NULL
                      ORDER BY last_active_at DESC LIMIT 1)
                 ),
                 context_window = COALESCE(
                     context_window,
                     (SELECT context_window FROM agent_chat_sessions
                      WHERE sdk_session_id = ?1 AND context_window IS NOT NULL
                      ORDER BY last_active_at DESC LIMIT 1)
                 ),
                 permission_mode = COALESCE(
                     permission_mode,
                     (SELECT permission_mode FROM agent_chat_sessions
                      WHERE sdk_session_id = ?1 AND permission_mode IS NOT NULL
                      ORDER BY last_active_at DESC LIMIT 1)
                 ),
                 fast_mode = COALESCE(
                     fast_mode,
                     (SELECT fast_mode FROM agent_chat_sessions
                      WHERE sdk_session_id = ?1 AND fast_mode IS NOT NULL
                      ORDER BY last_active_at DESC LIMIT 1),
                     0
                 )
             WHERE thread_id = ?2",
            params![sdk_session_id, survivor],
        )
        .map_err(|e| format!("Failed to merge duplicate sessions: {e}"))?;
        // Move persisted messages from the non-survivors onto the
        // survivor BEFORE the DELETE — otherwise ON DELETE CASCADE
        // wipes the transcript history of the prior thread_ids whose
        // messages we want to keep visible after resume.
        transaction.execute(
            "UPDATE agent_chat_messages
             SET thread_id = ?2
             WHERE thread_id IN (
                 SELECT thread_id FROM agent_chat_sessions
                 WHERE sdk_session_id = ?1 AND thread_id != ?2
             )",
            params![sdk_session_id, survivor],
        )
        .map_err(|e| format!("Failed to migrate duplicate session messages: {e}"))?;
        // Delete the non-survivors.
        transaction.execute(
            "DELETE FROM agent_chat_sessions
             WHERE sdk_session_id = ?1 AND thread_id != ?2",
            params![sdk_session_id, survivor],
        )
        .map_err(|e| format!("Failed to drop duplicate sessions: {e}"))?;
        transaction
            .commit()
            .map_err(|e| format!("Failed to commit duplicate-session merge: {e}"))?;
        Ok(cleanup)
    }

    /// Delete a session row. Idempotent.
    pub fn delete_agent_chat_session(&self, thread_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM agent_chat_sessions WHERE thread_id = ?1",
            params![thread_id],
        )
        .map_err(|e| format!("Failed to delete session: {e}"))?;
        Ok(())
    }

    /// Fetch a single session record by thread_id.
    pub fn get_agent_chat_session(&self, thread_id: &str) -> Option<AgentChatSessionRecord> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT thread_id, sdk_session_id, workspace_id, cwd, provider, title, created_at, last_active_at, model, effort, context_window, permission_mode, fast_mode
             FROM agent_chat_sessions WHERE thread_id = ?1",
            params![thread_id],
            |row| {
                Ok(AgentChatSessionRecord {
                    thread_id: row.get(0)?,
                    sdk_session_id: row.get(1)?,
                    workspace_id: row.get(2)?,
                    cwd: row.get(3)?,
                    provider: row.get(4)?,
                    title: row.get(5)?,
                    created_at: row.get(6)?,
                    last_active_at: row.get(7)?,
                    model: row.get(8)?,
                    effort: row.get(9)?,
                    context_window: row.get(10)?,
                    permission_mode: row.get(11)?,
                    fast_mode: row.get::<_, Option<i64>>(12)?.unwrap_or(0) != 0,
                })
            },
        )
        .ok()
    }
}

// ── Agent Chat Messages ──
//
// Per-message persistence so the SessionSelector "Resume" path can
// rehydrate the visible transcript, not just the SDK's server-side
// context. Rows are JSON-serialized envelopes — either a canonical
// `ProviderRuntimeEvent` or a synthetic `{type: "user_message", ...}`
// record (since user messages live only on the client side and never
// pass through the provider stream). Frontend hydration replays each
// payload through the same reducer that handles live events, so the
// rebuilt transcript is byte-identical to the original render.
//
// FK to agent_chat_sessions(thread_id) ON DELETE CASCADE means an
// explicit `delete_agent_chat_session` cleanly drops history. The two
// silent-restart / collapse paths above explicitly MOVE messages onto
// the survivor row before the delete, so logical-conversation merges
// never lose history.

impl DatabaseStore {
    /// Append a single message envelope. `payload_json` must be a
    /// fully-serialized JSON string — typically the result of
    /// `serde_json::to_string(&ProviderRuntimeEvent::…)` or a synthetic
    /// `{"type":"user_message","text":"…"}` record. Best-effort: rows
    /// for an unknown thread_id (FK violation) are silently dropped
    /// rather than crashing the event-bridge loop.
    ///
    /// Returns the inserted rowid — the durable, table-wide monotonic
    /// cursor position the frontend tracks as `lastPersistedEventId` so
    /// a remount can resume with `list_agent_chat_messages_after`
    /// instead of replaying from row zero. `Ok(None)` is the swallowed
    /// FK-violation case: nothing was written, so there is no position
    /// to advance to.
    pub fn append_agent_chat_message(
        &self,
        thread_id: &str,
        payload_json: &str,
    ) -> Result<Option<i64>, String> {
        let conn = self.conn.lock().unwrap();
        match conn.execute(
            "INSERT INTO agent_chat_messages (thread_id, payload, created_at)
             VALUES (?1, ?2, strftime('%Y-%m-%d %H:%M:%f', 'now'))",
            params![thread_id, payload_json],
        ) {
            Ok(_) => Ok(Some(conn.last_insert_rowid())),
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY =>
            {
                // Parent session row was already deleted (rare race
                // between forward_event and delete_agent_chat_session).
                // Drop the message silently — it is exactly the
                // history the user just asked us to forget.
                Ok(None)
            }
            Err(e) => Err(format!("Failed to append agent_chat_message: {e}")),
        }
    }

    /// Return every persisted message for a thread, ordered by
    /// insertion time (the autoincrement `id`). Each element is the
    /// raw JSON payload — the frontend parses and dispatches it.
    pub fn list_agent_chat_messages(&self, thread_id: &str) -> Vec<String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT payload FROM agent_chat_messages
             WHERE thread_id = ?1
             ORDER BY id ASC",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map(params![thread_id], |row| row.get::<_, String>(0))
            .map(|iter| iter.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// Cursor read: every message for `thread_id` with `id > after_id`,
    /// ordered ascending, as `(id, payload, created_at_ms)` triples. `None` means "from
    /// the beginning" (ids start at 1, so the query uses 0).
    ///
    /// Served entirely by `idx_agent_chat_messages_thread(thread_id, id
    /// ASC)` — a warm revisit with nothing new touches only the index
    /// tail, which is the whole point of the cursor.
    pub fn list_agent_chat_messages_after(
        &self,
        thread_id: &str,
        after_id: Option<i64>,
    ) -> Vec<(i64, String, i64)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT id, payload,
                    CAST((julianday(created_at) - 2440587.5) * 86400000 AS INTEGER)
             FROM agent_chat_messages
             WHERE thread_id = ?1 AND id > ?2
             ORDER BY id ASC",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map(params![thread_id, after_id.unwrap_or(0)], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map(|iter| iter.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    /// Highest row id currently stored for a thread, or `None` when the
    /// thread has no messages. The frontend compares this against its
    /// warm cursor: a cursor ABOVE the head means the cursor came from a
    /// different id space (a merged / deleted thread whose rows were
    /// re-homed by `collapse_duplicate_agent_chat_sessions`), so the
    /// tail read would silently return nothing forever. That case falls
    /// back to a cold hydrate.
    pub fn max_agent_chat_message_id(&self, thread_id: &str) -> Option<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT MAX(id) FROM agent_chat_messages WHERE thread_id = ?1",
            params![thread_id],
            |row| row.get::<_, Option<i64>>(0),
        )
        .ok()
        .flatten()
    }

    /// Fetch one message payload by rowid, verbatim. Backs the lazy
    /// tool-result fetch: the list read may hand the frontend a stub in
    /// place of a multi-megabyte `content`, and this is how the full
    /// body is retrieved on expand.
    pub fn get_agent_chat_message(&self, row_id: i64) -> Option<String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT payload FROM agent_chat_messages WHERE id = ?1",
            params![row_id],
            |row| row.get::<_, String>(0),
        )
        .ok()
    }

    /// Drop every persisted message for a thread without touching the
    /// session row. Used when the user picks "New Chat" against an
    /// existing thread (we'd rather not pollute history with the
    /// prior conversation's messages once they've been explicitly
    /// abandoned). Idempotent.
    #[cfg(test)]
    pub fn delete_agent_chat_messages_for_thread(&self, thread_id: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM agent_chat_messages WHERE thread_id = ?1",
            params![thread_id],
        )
        .map_err(|e| format!("Failed to delete messages: {e}"))?;
        Ok(())
    }
}

// ── Agent Chat Checkpoints (issue #80) ──
//
// One row per thread: the background run-start snapshot. Writes come
// from the checkpoint background task; reads from the pane header's
// restore affordance. Rows die with the session (FK CASCADE) or when
// the shadow ref is pruned (`delete_agent_chat_checkpoints_by_refs`).

impl DatabaseStore {
    /// Insert or replace the checkpoint row for a thread. A thread has
    /// at most one run-start checkpoint, so conflict = full replace.
    pub fn upsert_agent_chat_checkpoint(
        &self,
        record: &AgentChatCheckpointRecord,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO agent_chat_checkpoints
                 (thread_id, workspace_id, repo_path, ref_name,
                  snapshot_commit, head_commit, branch, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'))
             ON CONFLICT(thread_id) DO UPDATE SET
                 workspace_id = ?2,
                 repo_path = ?3,
                 ref_name = ?4,
                 snapshot_commit = ?5,
                 head_commit = ?6,
                 branch = ?7,
                 created_at = datetime('now')",
            params![
                record.thread_id,
                record.workspace_id,
                record.repo_path,
                record.ref_name,
                record.snapshot_commit,
                record.head_commit,
                record.branch,
            ],
        )
        .map_err(|e| format!("Failed to upsert agent_chat_checkpoint: {e}"))?;
        Ok(())
    }

    /// Fetch the checkpoint recorded for a thread, if any.
    pub fn get_agent_chat_checkpoint(&self, thread_id: &str) -> Option<AgentChatCheckpointRecord> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT thread_id, workspace_id, repo_path, ref_name,
                    snapshot_commit, head_commit, branch, created_at
             FROM agent_chat_checkpoints WHERE thread_id = ?1",
            params![thread_id],
            |row| {
                Ok(AgentChatCheckpointRecord {
                    thread_id: row.get(0)?,
                    workspace_id: row.get(1)?,
                    repo_path: row.get(2)?,
                    ref_name: row.get(3)?,
                    snapshot_commit: row.get(4)?,
                    head_commit: row.get(5)?,
                    branch: row.get(6)?,
                    created_at: row.get(7)?,
                })
            },
        )
        .optional()
        .ok()
        .flatten()
    }

    /// Drop the bookkeeping rows whose shadow refs were just pruned
    /// from a repo. `ref_name` alone is ambiguous across repos (two
    /// repos can both have `refs/codemux/checkpoints/x`), so the
    /// delete is scoped to the repo path.
    pub fn delete_agent_chat_checkpoints_by_refs(
        &self,
        repo_path: &str,
        ref_names: &[String],
    ) -> Result<(), String> {
        if ref_names.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().unwrap();
        for ref_name in ref_names {
            conn.execute(
                "DELETE FROM agent_chat_checkpoints
                 WHERE repo_path = ?1 AND ref_name = ?2",
                params![repo_path, ref_name],
            )
            .map_err(|e| format!("Failed to delete pruned checkpoint rows: {e}"))?;
        }
        Ok(())
    }
}

// ── Agent Chat Turn Checkpoints ──

fn row_to_agent_chat_turn_checkpoint(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<AgentChatTurnCheckpointRecord> {
    Ok(AgentChatTurnCheckpointRecord {
        thread_id: row.get(0)?,
        workspace_id: row.get(1)?,
        repo_path: row.get(2)?,
        turn_index: row.get(3)?,
        client_nonce: row.get(4)?,
        transcript_cutoff_id: row.get(5)?,
        ref_name: row.get(6)?,
        snapshot_commit: row.get(7)?,
        head_commit: row.get(8)?,
        branch: row.get(9)?,
        created_at: row.get(10)?,
    })
}

impl DatabaseStore {
    pub fn next_agent_chat_turn_checkpoint_index(&self, thread_id: &str) -> Result<i64, String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COALESCE(MAX(turn_index), 0) + 1
             FROM agent_chat_turn_checkpoints WHERE thread_id = ?1",
            params![thread_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("Failed to allocate turn checkpoint index: {error}"))
    }

    pub fn agent_chat_transcript_cutoff(&self, thread_id: &str) -> Result<i64, String> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COALESCE(MAX(id), 0) FROM agent_chat_messages WHERE thread_id = ?1",
            params![thread_id],
            |row| row.get(0),
        )
        .map_err(|error| format!("Failed to read transcript cutoff: {error}"))
    }

    pub fn upsert_agent_chat_turn_checkpoint(
        &self,
        record: &AgentChatTurnCheckpointRecord,
    ) -> Result<AgentChatTurnCheckpointRecord, String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO agent_chat_turn_checkpoints
                 (thread_id, turn_index, workspace_id, repo_path, client_nonce,
                  transcript_cutoff_id, ref_name, snapshot_commit, head_commit,
                  branch, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, datetime('now'))
             ON CONFLICT(thread_id, turn_index) DO UPDATE SET
                 workspace_id = excluded.workspace_id,
                 repo_path = excluded.repo_path,
                 client_nonce = excluded.client_nonce,
                 transcript_cutoff_id = excluded.transcript_cutoff_id,
                 ref_name = excluded.ref_name,
                 snapshot_commit = excluded.snapshot_commit,
                 head_commit = excluded.head_commit,
                 branch = excluded.branch,
                 created_at = datetime('now')",
            params![
                record.thread_id,
                record.turn_index,
                record.workspace_id,
                record.repo_path,
                record.client_nonce,
                record.transcript_cutoff_id,
                record.ref_name,
                record.snapshot_commit,
                record.head_commit,
                record.branch,
            ],
        )
        .map_err(|error| format!("Failed to persist turn checkpoint: {error}"))?;
        conn.query_row(
            "SELECT thread_id, workspace_id, repo_path, turn_index, client_nonce,
                    transcript_cutoff_id, ref_name, snapshot_commit, head_commit,
                    branch, created_at
             FROM agent_chat_turn_checkpoints
             WHERE thread_id = ?1 AND turn_index = ?2",
            params![record.thread_id, record.turn_index],
            row_to_agent_chat_turn_checkpoint,
        )
        .map_err(|error| format!("Failed to re-read turn checkpoint: {error}"))
    }

    /// Bind a committed checkpoint to the durable user-message row that
    /// materialized its accepted turn. The row immediately before that user
    /// message is the exact transcript boundary retained by revert.
    pub fn bind_agent_chat_turn_checkpoint_transcript(
        &self,
        thread_id: &str,
        client_nonce: &str,
        user_message_id: i64,
    ) -> Result<Option<AgentChatTurnCheckpointRecord>, String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agent_chat_turn_checkpoints
             SET transcript_cutoff_id = ?3
             WHERE thread_id = ?1 AND turn_index = (
                 SELECT MAX(turn_index) FROM agent_chat_turn_checkpoints
                 WHERE thread_id = ?1 AND client_nonce = ?2
             )",
            params![thread_id, client_nonce, user_message_id - 1],
        )
        .map_err(|error| format!("Failed to bind turn checkpoint transcript: {error}"))?;
        conn.query_row(
            "SELECT thread_id, workspace_id, repo_path, turn_index, client_nonce,
                    transcript_cutoff_id, ref_name, snapshot_commit, head_commit,
                    branch, created_at
             FROM agent_chat_turn_checkpoints
             WHERE thread_id = ?1 AND client_nonce = ?2
             ORDER BY turn_index DESC LIMIT 1",
            params![thread_id, client_nonce],
            row_to_agent_chat_turn_checkpoint,
        )
        .optional()
        .map_err(|error| format!("Failed to read bound turn checkpoint: {error}"))
    }

    pub fn list_agent_chat_turn_checkpoints(
        &self,
        thread_id: &str,
    ) -> Vec<AgentChatTurnCheckpointRecord> {
        let conn = self.conn.lock().unwrap();
        let mut statement = match conn.prepare(
            "SELECT thread_id, workspace_id, repo_path, turn_index, client_nonce,
                    transcript_cutoff_id, ref_name, snapshot_commit, head_commit,
                    branch, created_at
             FROM agent_chat_turn_checkpoints
             WHERE thread_id = ?1 ORDER BY turn_index ASC",
        ) {
            Ok(statement) => statement,
            Err(_) => return Vec::new(),
        };
        statement
            .query_map(params![thread_id], row_to_agent_chat_turn_checkpoint)
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    /// Cheap hot-path probe used when a turn is dispatched without checkpoint
    /// support. Avoid materializing as many as 500 timeline rows merely to
    /// decide whether stale refs need invalidating.
    pub fn has_agent_chat_turn_checkpoints(&self, thread_id: &str) -> bool {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM agent_chat_turn_checkpoints
                 WHERE thread_id = ?1 LIMIT 1
             )",
            params![thread_id],
            |row| row.get::<_, bool>(0),
        )
        .unwrap_or(false)
    }

    /// Invalidate the entire contiguous timeline, returning refs for cleanup.
    pub fn clear_agent_chat_turn_checkpoints(
        &self,
        thread_id: &str,
    ) -> Result<Vec<(String, String)>, String> {
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn
            .transaction()
            .map_err(|error| format!("Failed to begin checkpoint invalidation: {error}"))?;
        let refs = {
            let mut statement = transaction
                .prepare(
                    "SELECT repo_path, ref_name FROM agent_chat_turn_checkpoints
                     WHERE thread_id = ?1",
                )
                .map_err(|error| format!("Failed to list invalidated refs: {error}"))?;
            let refs = statement
                .query_map(params![thread_id], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|error| format!("Failed to read invalidated refs: {error}"))?
                .filter_map(Result::ok)
                .collect::<Vec<_>>();
            refs
        };
        transaction
            .execute(
                "DELETE FROM agent_chat_turn_checkpoints WHERE thread_id = ?1",
                params![thread_id],
            )
            .map_err(|error| format!("Failed to invalidate turn checkpoints: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit checkpoint invalidation: {error}"))?;
        Ok(refs)
    }

    /// Keep only the newest `keep` checkpoints for a thread, returning the
    /// shadow refs belonging to rows removed from the oldest edge.
    pub fn prune_agent_chat_turn_checkpoints(
        &self,
        thread_id: &str,
        keep: usize,
    ) -> Result<Vec<(String, String)>, String> {
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn
            .transaction()
            .map_err(|error| format!("Failed to begin turn checkpoint prune: {error}"))?;
        let stale = {
            let mut statement = transaction
                .prepare(
                    "SELECT turn_index, repo_path, ref_name
                     FROM agent_chat_turn_checkpoints
                     WHERE thread_id = ?1
                     ORDER BY turn_index DESC LIMIT -1 OFFSET ?2",
                )
                .map_err(|error| format!("Failed to prepare turn checkpoint prune: {error}"))?;
            let rows = statement
                .query_map(params![thread_id, keep as i64], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get(1)?, row.get(2)?))
                })
                .map_err(|error| format!("Failed to list old turn checkpoints: {error}"))?
                .filter_map(Result::ok)
                .collect::<Vec<_>>();
            rows
        };
        for (turn_index, _, _) in &stale {
            transaction
                .execute(
                    "DELETE FROM agent_chat_turn_checkpoints
                     WHERE thread_id = ?1 AND turn_index = ?2",
                    params![thread_id, turn_index],
                )
                .map_err(|error| format!("Failed to prune turn checkpoint: {error}"))?;
        }
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit turn checkpoint prune: {error}"))?;
        Ok(stale
            .into_iter()
            .map(|(_, repo_path, ref_name)| (repo_path, ref_name))
            .collect())
    }

    /// Atomically trim the local transcript and checkpoint timeline after the
    /// provider rollback succeeds. Returns what the caller must clean up
    /// outside SQLite (see [`AgentChatTurnRevertOutcome`]).
    pub fn finalize_agent_chat_turn_revert(
        &self,
        thread_id: &str,
        turn_index: i64,
        transcript_cutoff_id: i64,
    ) -> Result<AgentChatTurnRevertOutcome, String> {
        let mut conn = self.conn.lock().unwrap();
        let transaction = conn
            .transaction()
            .map_err(|error| format!("Failed to begin turn revert: {error}"))?;
        let refs = {
            let mut statement = transaction
                .prepare(
                    "SELECT repo_path, ref_name FROM agent_chat_turn_checkpoints
                     WHERE thread_id = ?1 AND turn_index >= ?2
                     ORDER BY turn_index ASC",
                )
                .map_err(|error| format!("Failed to list reverted refs: {error}"))?;
            let refs = statement
                .query_map(params![thread_id, turn_index], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .map_err(|error| format!("Failed to read reverted refs: {error}"))?
                .filter_map(Result::ok)
                .collect::<Vec<_>>();
            refs
        };
        // Attachment payloads on both sides of the cutoff. Restricted to rows
        // that actually carry an `images` array so a long transcript is never
        // materialized; the caller diffs them to find files nothing
        // references any more.
        let image_payloads = |cutoff_clause: &str| -> Result<Vec<String>, String> {
            let mut statement = transaction
                .prepare(&format!(
                    "SELECT payload FROM agent_chat_messages
                     WHERE thread_id = ?1 AND id {cutoff_clause} ?2
                       AND payload LIKE '%\"images\":[%'"
                ))
                .map_err(|error| format!("Failed to list reverted attachments: {error}"))?;
            let payloads = statement
                .query_map(params![thread_id, transcript_cutoff_id], |row| row.get(0))
                .map_err(|error| format!("Failed to read reverted attachments: {error}"))?
                .filter_map(Result::ok)
                .collect::<Vec<String>>();
            Ok(payloads)
        };
        let removed_image_payloads = image_payloads(">")?;
        let retained_image_payloads = image_payloads("<=")?;
        transaction
            .execute(
                "DELETE FROM agent_chat_messages WHERE thread_id = ?1 AND id > ?2",
                params![thread_id, transcript_cutoff_id],
            )
            .map_err(|error| format!("Failed to trim reverted transcript: {error}"))?;
        transaction
            .execute(
                "DELETE FROM agent_chat_turn_checkpoints
                 WHERE thread_id = ?1 AND turn_index >= ?2",
                params![thread_id, turn_index],
            )
            .map_err(|error| format!("Failed to trim reverted checkpoints: {error}"))?;
        transaction
            .commit()
            .map_err(|error| format!("Failed to commit turn revert: {error}"))?;
        Ok(AgentChatTurnRevertOutcome {
            removed_refs: refs,
            removed_image_payloads,
            retained_image_payloads,
        })
    }
}

// ── Auth Tokens ──

impl DatabaseStore {
    pub fn save_auth_token(&self, encrypted_data: &[u8]) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO auth_tokens (id, encrypted_data, updated_at) VALUES (1, ?1, datetime('now'))",
            params![encrypted_data],
        )
        .map_err(|e| format!("Failed to save auth token: {e}"))?;
        Ok(())
    }

    pub fn load_auth_token(&self) -> Option<Vec<u8>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT encrypted_data FROM auth_tokens WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .ok()
    }

    pub fn clear_auth_token(&self) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute("DELETE FROM auth_tokens WHERE id = 1", []);
    }
}

// ── Web remote access sessions ──
//
// Persistence for paired browser devices. The auth layer
// (`web_remote::auth`) owns token generation, hashing and the
// constant-time compare; these methods are the storage primitives it
// builds on. All rows are `user_id`-agnostic (single local user, like
// the rest of the local desktop state).
impl DatabaseStore {
    fn row_to_web_remote_session(
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<WebRemoteSessionRecord> {
        Ok(WebRemoteSessionRecord {
            id: row.get(0)?,
            name: row.get(1)?,
            user_agent: row.get(2)?,
            token_hash: row.get(3)?,
            created_at: row.get(4)?,
            last_seen_at: row.get(5)?,
            approved: row.get::<_, i64>(6)? != 0,
            revoked: row.get::<_, i64>(7)? != 0,
            source: row.get(8)?,
            account_user_id: row.get(9)?,
        })
    }

    /// Insert a freshly paired device. `token_hash` is the SHA-256 hex of
    /// the plaintext bearer token (which the caller keeps only long enough
    /// to hand back to the browser once).
    pub fn web_remote_insert_session(
        &self,
        id: &str,
        name: Option<&str>,
        user_agent: Option<&str>,
        token_hash: &str,
        approved: bool,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO web_remote_sessions
                (id, name, user_agent, token_hash, created_at, last_seen_at, approved, revoked, source)
             VALUES (?1, ?2, ?3, ?4, datetime('now'), datetime('now'), ?5, 0, 'pair')",
            params![id, name, user_agent, token_hash, approved as i64],
        )
        .map_err(|e| format!("Failed to insert web_remote session: {e}"))?;
        Ok(())
    }

    /// Insert a session admitted through account mode (`POST /api/pair-account`).
    /// Identical to [`web_remote_insert_session`] but tags the row
    /// `source = 'account'` and records the verified Codemux `account_user_id`
    /// (already checked to equal the desktop's own signed-in user), so the
    /// device list can distinguish account-minted from pairing-minted devices
    /// and the row carries an audit trail of which account admitted it.
    pub fn web_remote_insert_account_session(
        &self,
        id: &str,
        name: Option<&str>,
        user_agent: Option<&str>,
        token_hash: &str,
        approved: bool,
        account_user_id: &str,
    ) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO web_remote_sessions
                (id, name, user_agent, token_hash, created_at, last_seen_at, approved, revoked, source, account_user_id)
             VALUES (?1, ?2, ?3, ?4, datetime('now'), datetime('now'), ?5, 0, 'account', ?6)",
            params![id, name, user_agent, token_hash, approved as i64, account_user_id],
        )
        .map_err(|e| format!("Failed to insert web_remote account session: {e}"))?;
        Ok(())
    }

    /// Fetch a single session by id, regardless of revoked/approved state.
    pub fn web_remote_get_session(&self, id: &str) -> Option<WebRemoteSessionRecord> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, name, user_agent, token_hash, created_at, last_seen_at, approved, revoked, source, account_user_id
             FROM web_remote_sessions WHERE id = ?1",
            params![id],
            Self::row_to_web_remote_session,
        )
        .ok()
    }

    /// All non-revoked sessions, newest first. This is what the desktop
    /// device-management UI lists (pending devices included — the UI keys
    /// off `approved`).
    pub fn web_remote_list_sessions(&self) -> Vec<WebRemoteSessionRecord> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT id, name, user_agent, token_hash, created_at, last_seen_at, approved, revoked, source, account_user_id
             FROM web_remote_sessions WHERE revoked = 0 ORDER BY created_at DESC",
        ) {
            Ok(stmt) => stmt,
            Err(_) => return Vec::new(),
        };
        let rows = match stmt.query_map([], Self::row_to_web_remote_session) {
            Ok(rows) => rows,
            Err(_) => return Vec::new(),
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    /// Every non-revoked session, used by the auth layer to resolve a
    /// presented bearer token via a constant-time hash compare. Returns
    /// `(id, token_hash, approved)` tuples so the auth layer never has to
    /// materialise the whole record just to authenticate.
    pub fn web_remote_active_session_hashes(&self) -> Vec<(String, String, bool)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = match conn
            .prepare("SELECT id, token_hash, approved FROM web_remote_sessions WHERE revoked = 0")
        {
            Ok(stmt) => stmt,
            Err(_) => return Vec::new(),
        };
        let rows = match stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)? != 0,
            ))
        }) {
            Ok(rows) => rows,
            Err(_) => return Vec::new(),
        };
        rows.filter_map(|r| r.ok()).collect()
    }

    /// Bump `last_seen_at` to now. Best-effort; a failed touch is not fatal
    /// to a live connection.
    pub fn web_remote_touch_session(&self, id: &str) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "UPDATE web_remote_sessions SET last_seen_at = datetime('now') WHERE id = ?1",
            params![id],
        );
    }

    /// Flip a pending session to approved (approval-mode flow).
    pub fn web_remote_set_session_approved(&self, id: &str, approved: bool) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE web_remote_sessions SET approved = ?2 WHERE id = ?1",
            params![id, approved as i64],
        )
        .map_err(|e| format!("Failed to update web_remote session approval: {e}"))?;
        Ok(())
    }

    /// Revoke a session. The row is kept (revoked = 1) for UI history; the
    /// auth layer treats it as dead from this point on.
    pub fn web_remote_revoke_session(&self, id: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE web_remote_sessions SET revoked = 1 WHERE id = ?1",
            params![id],
        )
        .map_err(|e| format!("Failed to revoke web_remote session: {e}"))?;
        Ok(())
    }

    /// Hard-delete a session row. Used by the reject flow — a rejected
    /// pending device leaves no trace.
    pub fn web_remote_delete_session(&self, id: &str) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM web_remote_sessions WHERE id = ?1", params![id])
            .map_err(|e| format!("Failed to delete web_remote session: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_turn_checkpoint(
        thread_id: &str,
        turn_index: i64,
        nonce: &str,
        transcript_cutoff_id: i64,
    ) -> AgentChatTurnCheckpointRecord {
        AgentChatTurnCheckpointRecord {
            thread_id: thread_id.to_string(),
            workspace_id: "ws-checkpoint".to_string(),
            repo_path: "/tmp/checkpoint-repo".to_string(),
            turn_index,
            client_nonce: Some(nonce.to_string()),
            transcript_cutoff_id,
            ref_name: format!("refs/codemux/turn-checkpoints/{thread_id}/{turn_index}"),
            snapshot_commit: format!("snapshot-{turn_index}"),
            head_commit: "head".to_string(),
            branch: Some("main".to_string()),
            created_at: String::new(),
        }
    }

    #[test]
    fn turn_checkpoint_timeline_binds_prunes_and_finalizes_exactly() {
        let db = init_test_database();
        let thread_id = "turn-timeline";
        db.upsert_agent_chat_session(
            thread_id,
            "ws-checkpoint",
            Some("/tmp/checkpoint-repo"),
            "codex",
        )
        .unwrap();
        assert!(!db.has_agent_chat_turn_checkpoints(thread_id));

        db.upsert_agent_chat_turn_checkpoint(&sample_turn_checkpoint(
            thread_id,
            1,
            "nonce-1",
            0,
        ))
        .unwrap();
        let user_1 = db
            .append_agent_chat_message(
                thread_id,
                r#"{"type":"user_message","text":"one","images":[{"path":"/tmp/one.png"}]}"#,
            )
            .unwrap()
            .unwrap();
        let first = db
            .bind_agent_chat_turn_checkpoint_transcript(thread_id, "nonce-1", user_1)
            .unwrap()
            .unwrap();
        assert_eq!(first.transcript_cutoff_id, user_1 - 1);
        db.append_agent_chat_message(thread_id, r#"{"type":"turn_completed"}"#)
            .unwrap();

        db.upsert_agent_chat_turn_checkpoint(&sample_turn_checkpoint(
            thread_id,
            2,
            "nonce-2",
            db.agent_chat_transcript_cutoff(thread_id).unwrap(),
        ))
        .unwrap();
        let user_2 = db
            .append_agent_chat_message(
                thread_id,
                r#"{"type":"user_message","text":"two","images":[{"path":"/tmp/two.png"}]}"#,
            )
            .unwrap()
            .unwrap();
        let second = db
            .bind_agent_chat_turn_checkpoint_transcript(thread_id, "nonce-2", user_2)
            .unwrap()
            .unwrap();
        assert_eq!(second.transcript_cutoff_id, user_2 - 1);
        db.append_agent_chat_message(thread_id, r#"{"type":"turn_completed"}"#)
            .unwrap();

        db.upsert_agent_chat_turn_checkpoint(&sample_turn_checkpoint(
            thread_id,
            3,
            "nonce-3",
            db.agent_chat_transcript_cutoff(thread_id).unwrap(),
        ))
        .unwrap();
        let user_3 = db
            .append_agent_chat_message(
                thread_id,
                r#"{"type":"user_message","text":"three"}"#,
            )
            .unwrap()
            .unwrap();
        let third = db
            .bind_agent_chat_turn_checkpoint_transcript(thread_id, "nonce-3", user_3)
            .unwrap()
            .unwrap();
        db.append_agent_chat_message(thread_id, r#"{"type":"turn_completed"}"#)
            .unwrap();

        assert!(db.has_agent_chat_turn_checkpoints(thread_id));
        assert_eq!(
            db.next_agent_chat_turn_checkpoint_index(thread_id).unwrap(),
            4
        );
        assert!(db
            .bind_agent_chat_turn_checkpoint_transcript(thread_id, "missing", user_3)
            .unwrap()
            .is_none());

        let pruned = db.prune_agent_chat_turn_checkpoints(thread_id, 2).unwrap();
        assert_eq!(pruned, vec![(first.repo_path, first.ref_name)]);
        let retained = db.list_agent_chat_turn_checkpoints(thread_id);
        assert_eq!(
            retained
                .iter()
                .map(|checkpoint| checkpoint.turn_index)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );

        let removed = db
            .finalize_agent_chat_turn_revert(thread_id, 2, second.transcript_cutoff_id)
            .unwrap();
        assert_eq!(
            removed.removed_refs,
            vec![
                (second.repo_path, second.ref_name),
                (third.repo_path, third.ref_name),
            ]
        );
        // Attachment payloads are partitioned across the cutoff so the
        // command layer can unlink only the files nothing references now.
        assert_eq!(removed.removed_image_payloads.len(), 1);
        assert!(removed.removed_image_payloads[0].contains("/tmp/two.png"));
        assert_eq!(removed.retained_image_payloads.len(), 1);
        assert!(removed.retained_image_payloads[0].contains("/tmp/one.png"));
        assert_eq!(db.list_agent_chat_messages(thread_id).len(), 2);
        assert!(!db.has_agent_chat_turn_checkpoints(thread_id));
    }

    #[test]
    fn migration_drops_retired_orchestration_history() {
        let db = init_test_database();
        let conn = db.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TABLE openflow_history (id TEXT PRIMARY KEY);\n\
             INSERT INTO openflow_history (id) VALUES ('legacy-run');",
        )
        .unwrap();

        create_schema(&conn).unwrap();

        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type = 'table' AND name = 'openflow_history'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 0);
    }

    #[test]
    fn usage_ledger_round_trip_and_range_filter() {
        let db = init_test_database();
        let day = 86_400_000i64;
        let now = 1_800_000_000_000i64;

        db.insert_usage_row(
            now,
            "t1",
            Some("ws1"),
            "claude",
            Some("claude-opus-4-5"),
            false,
            100,
            20,
            3_000,
            500,
            0,
            Some(1.25),
            Some("table"),
        )
        .unwrap();
        // A subagent row on the same thread.
        db.insert_usage_row(
            now,
            "t1",
            Some("ws1"),
            "claude",
            Some("claude-haiku-4-5"),
            true,
            40,
            10,
            0,
            0,
            0,
            Some(0.01),
            Some("table"),
        )
        .unwrap();
        // A metered row in another workspace.
        db.insert_usage_row(
            now,
            "t2",
            Some("ws2"),
            "opencode",
            Some("openrouter/kimi-k2"),
            false,
            5,
            5,
            0,
            0,
            0,
            None,
            None,
        )
        .unwrap();
        // Well outside any window the dashboard asks for.
        db.insert_usage_row(
            now - 90 * day,
            "t9",
            Some("ws9"),
            "codex",
            None,
            false,
            999,
            999,
            0,
            0,
            0,
            Some(50.0),
            Some("table"),
        )
        .unwrap();

        let rows = db.usage_rows_since(now - 7 * day).unwrap();
        assert_eq!(rows.len(), 3, "the 90-day-old row is filtered out in SQL");

        let first = &rows[0];
        assert_eq!(first.thread_id, "t1");
        assert_eq!(first.workspace_id.as_deref(), Some("ws1"));
        assert_eq!(first.provider, "claude");
        assert_eq!(first.model.as_deref(), Some("claude-opus-4-5"));
        assert!(!first.subagent);
        assert_eq!(first.input_tokens, 100);
        assert_eq!(first.cache_read_tokens, 3_000);
        assert_eq!(first.cost_usd, Some(1.25));
        // The four buckets are disjoint, so the total is their sum.
        assert_eq!(first.total_tokens(), 3_620);

        // The subagent flag and a NULL model/cost both survive the trip.
        assert!(rows[1].subagent);
        assert!(rows[2].model.is_some());
        assert_eq!(rows[2].cost_usd, None);
    }

    /// The ledger must OUTLIVE the chat it came from. `agent_chat_messages`
    /// cascades on session delete; deleting a chat from the history
    /// dropdown must not rewrite last month's spend.
    /// v12 backfill: rows written before `cost_source` existed get
    /// labeled 'table'. Conservative by construction — an OpenCode row
    /// that was actually catalogue-priced is indistinguishable after the
    /// fact, so it is labeled 'table' too.
    #[test]
    fn migration_backfills_cost_source_for_priced_rows_only() {
        let db = init_test_database();
        let now = 1_800_000_000_000i64;
        {
            let conn = db.conn.lock().unwrap();
            // Simulate pre-v12 rows: written with a NULL cost_source.
            conn.execute(
                "INSERT INTO agent_usage_ledger
                   (created_at, thread_id, provider, model, subagent,
                    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                    cost_usd, cost_source)
                 VALUES (?1, 'legacy-priced', 'claude', 'm', 0, 10, 10, 0, 0, 1.5, NULL)",
                params![now],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO agent_usage_ledger
                   (created_at, thread_id, provider, model, subagent,
                    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                    cost_usd, cost_source)
                 VALUES (?1, 'legacy-unpriced', 'opencode', 'm', 0, 10, 10, 0, 0, NULL, NULL)",
                params![now],
            )
            .unwrap();
            // Re-run the migrations (idempotent) to fire the backfill.
            create_schema(&conn).unwrap();
        }

        let rows = db.usage_rows_since(0).unwrap();
        let priced = rows
            .iter()
            .find(|r| r.thread_id == "legacy-priced")
            .unwrap();
        let unpriced = rows
            .iter()
            .find(|r| r.thread_id == "legacy-unpriced")
            .unwrap();
        assert_eq!(priced.cost_source.as_deref(), Some("table"));
        // A row with no cost must stay unlabeled — labeling it would
        // claim a price that was never computed.
        assert!(unpriced.cost_source.is_none());
        // The new column defaults to 0 for pre-existing rows.
        assert_eq!(priced.reasoning_tokens, 0);
    }

    /// `reasoning_tokens` is a subset of output and must never inflate a
    /// total or the resume baseline's four billable terms.
    #[test]
    fn reasoning_round_trips_without_entering_the_total() {
        let db = init_test_database();
        db.insert_usage_row(
            1_800_000_000_000,
            "t1",
            Some("ws"),
            "codex",
            Some("gpt-5-codex"),
            false,
            10,
            40,
            0,
            0,
            25,
            Some(1.0),
            Some("table"),
        )
        .unwrap();
        let rows = db.usage_rows_since(0).unwrap();
        assert_eq!(rows[0].reasoning_tokens, 25);
        assert_eq!(
            rows[0].total_tokens(),
            50,
            "reasoning excluded from the sum"
        );
        assert_eq!(rows[0].observed_input(), 10);

        // …but the baseline DOES carry it, so a resumed Codex thread
        // does not re-record its reasoning history.
        let (i, o, cr, cw, reasoning) = db.recorded_usage_totals("t1", "codex").unwrap();
        assert_eq!((i, o, cr, cw), (10, 40, 0, 0));
        assert_eq!(reasoning, 25);
    }

    /// Re-scanning provider history is a no-op, while a growing response
    /// updates the same natural record instead of freezing a partial row.
    #[test]
    fn provider_rows_are_idempotent_and_growing_rows_update() {
        let db = init_test_database();
        let insert = || {
            db.upsert_provider_usage_row(
                "claude:sess-1:msg_A",
                1_800_000_000_000,
                "claude:sess-1",
                "claude",
                Some("claude-opus-4-5"),
                false,
                100,
                20,
                0,
                0,
                0,
                Some(1.0),
                Some("table"),
            )
        };
        assert!(insert().unwrap(), "first scan writes the row");
        assert!(!insert().unwrap(), "second scan is a no-op");
        assert_eq!(db.usage_rows_since(0).unwrap().len(), 1);

        assert!(db
            .upsert_provider_usage_row(
                "claude:sess-1:msg_A",
                1_800_000_000_001,
                "claude:sess-1",
                "claude",
                Some("claude-opus-4-5"),
                false,
                100,
                80,
                0,
                0,
                0,
                Some(2.0),
                Some("table"),
            )
            .unwrap());
        let grown = db.usage_rows_since(0).unwrap();
        assert_eq!(grown.len(), 1, "same provider record, still one row");
        assert_eq!(grown[0].output_tokens, 80);
        assert_eq!(grown[0].cost_usd, Some(2.0));

        // A different key is a different row.
        db.upsert_provider_usage_row(
            "claude:sess-1:msg_B",
            1_800_000_000_000,
            "claude:sess-1",
            "claude",
            Some("claude-opus-4-5"),
            false,
            5,
            5,
            0,
            0,
            0,
            None,
            None,
        )
        .unwrap();
        assert_eq!(db.usage_rows_since(0).unwrap().len(), 2);
    }

    #[test]
    fn grok_live_usage_is_idempotent_and_preserves_exact_or_unknown_cost() {
        let db = init_test_database();
        db.upsert_agent_chat_session("grok-thread", "ws-grok", Some("/repo"), "grok")
            .unwrap();

        assert!(db
            .upsert_grok_live_usage_row(
                "grok:grok-thread:turn-1",
                1_800_000_000_000,
                "grok-thread",
                Some("grok-future"),
                false,
                100,
                10,
                20,
                5,
                3,
                Some(0.25),
                Some("provider"),
            )
            .unwrap());
        // A replay of the same terminal turn upserts its stable key rather
        // than creating a second charge.
        db.upsert_grok_live_usage_row(
            "grok:grok-thread:turn-1",
            1_800_000_000_000,
            "grok-thread",
            Some("grok-future"),
            false,
            100,
            10,
            20,
            5,
            3,
            Some(0.25),
            Some("provider"),
        )
        .unwrap();
        // Missing cost is authoritative too: do not invent a table estimate.
        db.upsert_grok_live_usage_row(
            "grok:grok-thread:turn-2",
            1_800_000_000_001,
            "grok-thread",
            Some("grok-future"),
            false,
            7,
            2,
            0,
            0,
            0,
            None,
            None,
        )
        .unwrap();

        let rows = db.usage_rows_since(0).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].workspace_id.as_deref(), Some("ws-grok"));
        assert_eq!(rows[0].provider, "grok");
        assert_eq!(rows[0].source, "live");
        assert_eq!(rows[0].total_tokens(), 135);
        assert_eq!(rows[0].reasoning_tokens, 3);
        assert_eq!(rows[0].cost_usd, Some(0.25));
        assert_eq!(rows[0].cost_source.as_deref(), Some("provider"));
        assert_eq!(rows[1].cost_usd, None);
        assert_eq!(rows[1].cost_source, None);
    }

    /// A corrected importer rebuilds only its derived cache. Exact Grok live
    /// rows have no provider-history replacement and must survive.
    #[test]
    fn resetting_usage_history_preserves_exact_grok_live_rows() {
        let db = init_test_database();
        db.upsert_provider_usage_row(
            "claude:msg_A:req_1",
            1_800_000_000_000,
            "claude:sess-1",
            "claude",
            Some("claude-opus-4-5"),
            false,
            100,
            20,
            0,
            0,
            0,
            Some(1.0),
            Some("table"),
        )
        .unwrap();
        db.upsert_grok_live_usage_row(
            "grok:thread-live:turn-1",
            1_800_000_000_000,
            "thread-live",
            Some("grok-future"),
            false,
            10,
            40,
            0,
            0,
            0,
            Some(2.0),
            Some("provider"),
        )
        .unwrap();
        db.set_usage_import_state("/logs/a.jsonl", 1, 2, 2, 3)
            .unwrap();
        assert_eq!(db.usage_rows_since(0).unwrap().len(), 2);

        assert_eq!(
            db.reset_usage_history().unwrap(),
            1,
            "only the rebuildable provider-history row"
        );

        let rows = db.usage_rows_since(0).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].provider, "grok");
        assert_eq!(rows[0].source, "live");
        // The per-file state must go with it, or the rebuild scan would
        // skip every unchanged file and import nothing.
        assert!(db.usage_import_state("/logs/a.jsonl").unwrap().is_none());

        // And the same key can be re-imported afterwards.
        assert!(db
            .upsert_provider_usage_row(
                "claude:msg_A:req_1",
                1_800_000_000_000,
                "claude:sess-1",
                "claude",
                Some("claude-opus-4-5"),
                false,
                100,
                20,
                0,
                0,
                0,
                Some(3.0),
                Some("table"),
            )
            .unwrap());
    }

    #[test]
    fn provider_rows_are_tagged_and_carry_no_workspace() {
        let db = init_test_database();
        db.upsert_provider_usage_row(
            "codex:roll-1",
            1_800_000_000_000,
            "codex:roll-1",
            "codex",
            Some("gpt-5-codex"),
            false,
            10,
            40,
            0,
            0,
            25,
            Some(2.0),
            Some("table"),
        )
        .unwrap();
        let rows = db.usage_rows_since(0).unwrap();
        assert_eq!(rows[0].source, "provider_history");
        assert!(rows[0].workspace_id.is_none());
        assert_eq!(rows[0].reasoning_tokens, 25);
        assert_eq!(rows[0].cost_source.as_deref(), Some("table"));
    }

    #[test]
    fn provider_rows_batch_in_one_transaction() {
        let db = init_test_database();
        let make = |key: &str, output: i64| ProviderUsageCacheRow {
            import_key: key.to_string(),
            created_at: 1_800_000_000_000,
            thread_id: "opencode:session-1".into(),
            provider: "opencode".into(),
            model: Some("openai/gpt-5".into()),
            subagent: false,
            input_tokens: 10,
            output_tokens: output,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            cost_usd: Some(0.25),
            cost_source: Some("provider".into()),
        };
        let rows = [make("opencode:a", 2), make("opencode:b", 3)];
        assert_eq!(db.upsert_provider_usage_rows(&rows).unwrap(), 2);
        assert_eq!(db.usage_rows_since(0).unwrap().len(), 2);
        assert_eq!(db.upsert_provider_usage_rows(&rows).unwrap(), 0);
    }

    #[test]
    fn import_state_tracks_file_growth() {
        let db = init_test_database();
        assert!(db.usage_import_state("/tmp/a.jsonl").unwrap().is_none());
        db.set_usage_import_state("/tmp/a.jsonl", 111, 500, 500, 999)
            .unwrap();
        assert_eq!(
            db.usage_import_state("/tmp/a.jsonl").unwrap(),
            Some((111, 500, 500))
        );
        // Upsert, not a second row.
        db.set_usage_import_state("/tmp/a.jsonl", 222, 900, 900, 1000)
            .unwrap();
        assert_eq!(
            db.usage_import_state("/tmp/a.jsonl").unwrap(),
            Some((222, 900, 900))
        );
    }

    #[test]
    fn usage_rows_survive_deleting_their_chat_session() {
        let db = init_test_database();
        let now = 1_800_000_000_000i64;

        db.upsert_agent_chat_session("t1", "ws1", Some("/tmp"), "claude")
            .unwrap();
        db.insert_usage_row(
            now,
            "t1",
            Some("ws1"),
            "claude",
            Some("claude-opus-4-5"),
            false,
            10,
            10,
            0,
            0,
            0,
            Some(0.5),
            Some("table"),
        )
        .unwrap();

        db.delete_agent_chat_session("t1").unwrap();

        let rows = db.usage_rows_since(0).unwrap();
        assert_eq!(rows.len(), 1, "usage history must not cascade away");
        assert_eq!(rows[0].thread_id, "t1");
        assert_eq!(rows[0].cost_usd, Some(0.5));
    }

    /// A row for a thread that never had a session row is still valid —
    /// `workspace_id` is nullable and there is no foreign key.
    #[test]
    fn usage_row_accepts_a_null_workspace() {
        let db = init_test_database();
        db.insert_usage_row(
            1_800_000_000_000,
            "orphan",
            None,
            "opencode",
            None,
            false,
            1,
            1,
            0,
            0,
            0,
            None,
            None,
        )
        .unwrap();
        let rows = db.usage_rows_since(0).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].workspace_id.is_none());
    }

    #[test]
    fn settings_crud() {
        let db = init_test_database();

        // Get non-existent setting
        assert_eq!(db.get_setting("theme"), None);

        // Set and get
        db.set_setting("theme", "dark").unwrap();
        assert_eq!(db.get_setting("theme"), Some("dark".into()));

        // Update
        db.set_setting("theme", "light").unwrap();
        assert_eq!(db.get_setting("theme"), Some("light".into()));

        // Delete
        db.delete_setting("theme").unwrap();
        assert_eq!(db.get_setting("theme"), None);
    }

    #[test]
    fn backfill_null_permission_mode_migration() {
        let db = init_test_database();

        // Legacy rows: `upsert_agent_chat_session` never writes
        // permission_mode, so the column reads back NULL — exactly the
        // pre-column shape the migration backfills.
        db.upsert_agent_chat_session("t-claude", "ws", Some("/tmp"), "claude")
            .unwrap();
        db.upsert_agent_chat_session("t-codex", "ws", Some("/tmp"), "codex")
            .unwrap();
        db.upsert_agent_chat_session("t-opencode", "ws", Some("/tmp"), "opencode")
            .unwrap();
        // A claude row that already carries an explicit non-default mode
        // must survive the backfill untouched.
        db.upsert_agent_chat_session("t-claude-set", "ws", Some("/tmp"), "claude")
            .unwrap();
        db.update_agent_chat_session_config(
            "t-claude-set",
            &AgentChatSessionConfig {
                permission_mode: AgentChatSessionConfig::set("plan"),
                ..AgentChatSessionConfig::default()
            },
        )
        .unwrap();
        // v0.14.2 could also persist the other provider's Full-access
        // protocol value after a provider switch. These rows must be
        // canonicalized without touching unrelated explicit modes.
        db.upsert_agent_chat_session("t-codex-cross", "ws", Some("/tmp"), "codex")
            .unwrap();
        db.update_agent_chat_session_config(
            "t-codex-cross",
            &AgentChatSessionConfig {
                permission_mode: AgentChatSessionConfig::set("bypassPermissions"),
                ..AgentChatSessionConfig::default()
            },
        )
        .unwrap();
        db.upsert_agent_chat_session("t-claude-cross", "ws", Some("/tmp"), "claude")
            .unwrap();
        db.update_agent_chat_session_config(
            "t-claude-cross",
            &AgentChatSessionConfig {
                permission_mode: AgentChatSessionConfig::set("danger-full-access"),
                ..AgentChatSessionConfig::default()
            },
        )
        .unwrap();

        // Re-run the schema migrations (idempotent) to fire the backfill.
        {
            let conn = db.conn.lock().unwrap();
            create_schema(&conn).unwrap();
        }

        let mode = |thread: &str| db.get_agent_chat_session(thread).unwrap().permission_mode;
        assert_eq!(mode("t-claude").as_deref(), Some("bypassPermissions"));
        assert_eq!(mode("t-codex").as_deref(), Some("danger-full-access"));
        assert_eq!(mode("t-opencode"), None, "opencode rows stay NULL");
        assert_eq!(mode("t-codex-cross").as_deref(), Some("danger-full-access"));
        assert_eq!(mode("t-claude-cross").as_deref(), Some("bypassPermissions"));
        assert_eq!(
            mode("t-claude-set").as_deref(),
            Some("plan"),
            "rows with an explicit mode are untouched"
        );
    }

    #[test]
    fn get_all_settings() {
        let db = init_test_database();

        db.set_setting("a", "1").unwrap();
        db.set_setting("b", "2").unwrap();
        db.set_setting("c", "3").unwrap();

        let all = db.get_all_settings();
        assert_eq!(all.len(), 3);
        assert_eq!(all.get("a"), Some(&"1".into()));
        assert_eq!(all.get("b"), Some(&"2".into()));
        assert_eq!(all.get("c"), Some(&"3".into()));
    }

    #[test]
    fn ui_state_crud() {
        let db = init_test_database();

        assert_eq!(db.get_ui_state("sidebar_width"), None);

        db.set_ui_state("sidebar_width", "300").unwrap();
        assert_eq!(db.get_ui_state("sidebar_width"), Some("300".into()));

        // Upsert
        db.set_ui_state("sidebar_width", "250").unwrap();
        assert_eq!(db.get_ui_state("sidebar_width"), Some("250".into()));
    }

    #[test]
    fn recent_projects_crud() {
        let db = init_test_database();

        // Empty list
        assert_eq!(db.get_recent_projects(10).len(), 0);

        // Add projects
        db.add_recent_project("/home/user/project-a", "project-a")
            .unwrap();
        db.add_recent_project("/home/user/project-b", "project-b")
            .unwrap();

        let projects = db.get_recent_projects(10);
        assert_eq!(projects.len(), 2);

        // Both projects present
        let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"project-a"));
        assert!(names.contains(&"project-b"));

        // Upsert doesn't duplicate
        db.add_recent_project("/home/user/project-a", "project-a-renamed")
            .unwrap();
        let projects = db.get_recent_projects(10);
        assert_eq!(projects.len(), 2);
        // Name was updated
        assert!(projects.iter().any(|p| p.name == "project-a-renamed"));

        // Limit
        let projects = db.get_recent_projects(1);
        assert_eq!(projects.len(), 1);
    }

    #[test]
    fn schema_version_set() {
        let db = init_test_database();
        let conn = db.conn.lock().unwrap();
        let version: u32 = conn
            .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn schema_v14_upgrade_creates_new_tables_and_advances_to_current() {
        let db = init_test_database();
        let conn = db.conn.lock().unwrap();
        conn.execute_batch(
            "DROP TABLE agent_chat_turn_checkpoints;
             DROP TABLE agent_chat_handoff_summaries;
             UPDATE schema_version SET version = 14;",
        )
        .unwrap();

        create_schema(&conn).unwrap();

        let version: u32 = conn
            .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        // v15 added turn checkpoints, v16 the handoff summary cache; both are
        // plain `CREATE ... IF NOT EXISTS`, so one upgrade pass restores both.
        for table in ["agent_chat_turn_checkpoints", "agent_chat_handoff_summaries"] {
            let table_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(table_count, 1, "{table} should be recreated");
        }
    }

    // ── Settings Persistence ──

    #[test]
    fn settings_default_returns_none() {
        let db = init_test_database();
        // Non-existent settings return None (app should use defaults)
        assert_eq!(db.get_setting("nonexistent_key"), None);
        assert_eq!(db.get_setting("theme"), None);
        assert_eq!(db.get_setting("notification_sound_enabled"), None);
    }

    #[test]
    fn settings_multiple_independent() {
        let db = init_test_database();

        db.set_setting("theme", "dark").unwrap();
        db.set_setting("font_size", "14").unwrap();
        db.set_setting("ai_enabled", "true").unwrap();

        // Each setting is independent
        assert_eq!(db.get_setting("theme"), Some("dark".into()));
        assert_eq!(db.get_setting("font_size"), Some("14".into()));
        assert_eq!(db.get_setting("ai_enabled"), Some("true".into()));

        // Deleting one doesn't affect others
        db.delete_setting("font_size").unwrap();
        assert_eq!(db.get_setting("theme"), Some("dark".into()));
        assert_eq!(db.get_setting("font_size"), None);
        assert_eq!(db.get_setting("ai_enabled"), Some("true".into()));

        // get_all_settings reflects deletions
        let all = db.get_all_settings();
        assert_eq!(all.len(), 2);
        assert!(!all.contains_key("font_size"));
    }

    #[test]
    fn settings_delete_nonexistent_is_ok() {
        let db = init_test_database();
        // Deleting a non-existent key should not error
        assert!(db.delete_setting("does_not_exist").is_ok());
    }

    #[test]
    fn settings_empty_and_special_values() {
        let db = init_test_database();

        // Empty string value
        db.set_setting("empty", "").unwrap();
        assert_eq!(db.get_setting("empty"), Some("".into()));

        // Value with special characters
        db.set_setting("special", "hello \"world\" 'test' \n\t")
            .unwrap();
        assert_eq!(
            db.get_setting("special"),
            Some("hello \"world\" 'test' \n\t".into())
        );

        // Long value
        let long_val = "x".repeat(10_000);
        db.set_setting("long", &long_val).unwrap();
        assert_eq!(db.get_setting("long"), Some(long_val));
    }

    // ── UI State Persistence ──

    #[test]
    fn ui_state_collapse_states() {
        let db = init_test_database();

        // Save collapse states for multiple project groups
        db.set_ui_state("collapsed:project:/home/user/project-a", "true")
            .unwrap();
        db.set_ui_state("collapsed:project:/home/user/project-b", "false")
            .unwrap();

        assert_eq!(
            db.get_ui_state("collapsed:project:/home/user/project-a"),
            Some("true".into())
        );
        assert_eq!(
            db.get_ui_state("collapsed:project:/home/user/project-b"),
            Some("false".into())
        );
        assert_eq!(
            db.get_ui_state("collapsed:project:/home/user/project-c"),
            None
        );
    }

    #[test]
    fn ui_state_right_panel_width() {
        let db = init_test_database();

        db.set_ui_state("right_panel_width", "350").unwrap();
        assert_eq!(db.get_ui_state("right_panel_width"), Some("350".into()));

        // Update width
        db.set_ui_state("right_panel_width", "280").unwrap();
        assert_eq!(db.get_ui_state("right_panel_width"), Some("280".into()));
    }

    #[test]
    fn ui_state_active_workspace() {
        let db = init_test_database();

        db.set_ui_state("active_workspace", "workspace-abc123")
            .unwrap();
        assert_eq!(
            db.get_ui_state("active_workspace"),
            Some("workspace-abc123".into())
        );

        // Switch workspace
        db.set_ui_state("active_workspace", "workspace-def456")
            .unwrap();
        assert_eq!(
            db.get_ui_state("active_workspace"),
            Some("workspace-def456".into())
        );
    }

    #[test]
    fn ui_state_window_dimensions() {
        let db = init_test_database();

        db.set_ui_state("window_width", "1920").unwrap();
        db.set_ui_state("window_height", "1080").unwrap();
        db.set_ui_state("window_x", "100").unwrap();
        db.set_ui_state("window_y", "50").unwrap();

        assert_eq!(db.get_ui_state("window_width"), Some("1920".into()));
        assert_eq!(db.get_ui_state("window_height"), Some("1080".into()));
        assert_eq!(db.get_ui_state("window_x"), Some("100".into()));
        assert_eq!(db.get_ui_state("window_y"), Some("50".into()));

        // Update dimensions (window resized)
        db.set_ui_state("window_width", "2560").unwrap();
        db.set_ui_state("window_height", "1440").unwrap();
        assert_eq!(db.get_ui_state("window_width"), Some("2560".into()));
        assert_eq!(db.get_ui_state("window_height"), Some("1440".into()));
    }

    #[test]
    fn ui_state_overwrite_existing() {
        let db = init_test_database();

        db.set_ui_state("key", "value1").unwrap();
        assert_eq!(db.get_ui_state("key"), Some("value1".into()));

        db.set_ui_state("key", "value2").unwrap();
        assert_eq!(db.get_ui_state("key"), Some("value2".into()));

        db.set_ui_state("key", "value3").unwrap();
        assert_eq!(db.get_ui_state("key"), Some("value3".into()));
    }

    // ── Recent Projects ──

    #[test]
    fn recent_projects_ordering() {
        let db = init_test_database();

        // Add 5 projects with explicit timestamps via raw SQL to control ordering
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO recent_projects (user_id, path, name, last_opened_at) VALUES ('local', '/p1', 'p1', '2025-01-01')",
                [],
            ).unwrap();
            conn.execute(
                "INSERT INTO recent_projects (user_id, path, name, last_opened_at) VALUES ('local', '/p2', 'p2', '2025-01-03')",
                [],
            ).unwrap();
            conn.execute(
                "INSERT INTO recent_projects (user_id, path, name, last_opened_at) VALUES ('local', '/p3', 'p3', '2025-01-02')",
                [],
            ).unwrap();
        }

        let projects = db.get_recent_projects(10);
        assert_eq!(projects.len(), 3);
        // Most recent first
        assert_eq!(projects[0].name, "p2"); // 2025-01-03
        assert_eq!(projects[1].name, "p3"); // 2025-01-02
        assert_eq!(projects[2].name, "p1"); // 2025-01-01
    }

    #[test]
    fn recent_projects_many() {
        let db = init_test_database();

        // Add 20 projects
        for i in 0..20 {
            db.add_recent_project(
                &format!("/home/user/project-{i:02}"),
                &format!("project-{i:02}"),
            )
            .unwrap();
        }

        let all = db.get_recent_projects(100);
        assert_eq!(all.len(), 20);

        // Limit works
        let limited = db.get_recent_projects(5);
        assert_eq!(limited.len(), 5);

        // All unique paths
        let paths: std::collections::HashSet<&str> = all.iter().map(|p| p.path.as_str()).collect();
        assert_eq!(paths.len(), 20);
    }

    // ── Integration / Lifecycle ──

    #[test]
    fn lifecycle_file_persistence() {
        // Test that data survives closing and reopening the database
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        // Phase 1: Create DB and write data
        {
            let conn = open_connection(&db_path).unwrap();
            create_schema(&conn).unwrap();
            let db = DatabaseStore {
                conn: Mutex::new(conn),
            };

            db.set_setting("theme", "dark").unwrap();
            db.set_ui_state("window_width", "1920").unwrap();
            db.add_recent_project("/home/user/myapp", "myapp").unwrap();
        }
        // Connection dropped here

        // Phase 2: Reopen DB and verify data survived
        {
            let conn = open_connection(&db_path).unwrap();
            create_schema(&conn).unwrap(); // Should be idempotent
            let db = DatabaseStore {
                conn: Mutex::new(conn),
            };

            assert_eq!(db.get_setting("theme"), Some("dark".into()));
            assert_eq!(db.get_ui_state("window_width"), Some("1920".into()));

            let projects = db.get_recent_projects(10);
            assert_eq!(projects.len(), 1);
            assert_eq!(projects[0].name, "myapp");
        }
    }

    #[test]
    fn schema_creation_is_idempotent() {
        // Running create_schema multiple times should not error or lose data
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();

        create_schema(&conn).unwrap();

        // Insert data
        conn.execute(
            "INSERT INTO settings (user_id, key, value) VALUES ('local', 'test', 'hello')",
            [],
        )
        .unwrap();

        // Run schema creation again
        create_schema(&conn).unwrap();

        // Data preserved
        let val: String = conn
            .query_row("SELECT value FROM settings WHERE key = 'test'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(val, "hello");
    }

    #[test]
    fn concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let db = Arc::new(init_test_database());
        let mut handles = vec![];

        // Spawn 10 threads that each write and read settings
        for i in 0..10 {
            let db = Arc::clone(&db);
            handles.push(thread::spawn(move || {
                let key = format!("thread_{i}");
                let value = format!("value_{i}");
                db.set_setting(&key, &value).unwrap();
                let read = db.get_setting(&key);
                assert_eq!(read, Some(value));
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // All 10 settings should exist
        let all = db.get_all_settings();
        assert_eq!(all.len(), 10);
        for i in 0..10 {
            assert_eq!(all.get(&format!("thread_{i}")), Some(&format!("value_{i}")));
        }
    }

    #[test]
    fn concurrent_ui_state_access() {
        use std::sync::Arc;
        use std::thread;

        let db = Arc::new(init_test_database());
        let mut handles = vec![];

        // Multiple threads updating the same key
        for i in 0..20 {
            let db = Arc::clone(&db);
            handles.push(thread::spawn(move || {
                db.set_ui_state("counter", &i.to_string()).unwrap();
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        // Key should exist with some value (last writer wins)
        let val = db.get_ui_state("counter");
        assert!(val.is_some());
        let num: i32 = val.unwrap().parse().unwrap();
        assert!((0..20).contains(&num));
    }

    // ── E2E Wiring Tests ──

    #[test]
    fn wiring_settings_roundtrip() {
        // Simulates: settings panel writes → SQLite → app startup reads
        let db = init_test_database();

        // 1. "Settings panel" saves values (what dbSetSetting does)
        db.set_setting("notification_sound_enabled", "true")
            .unwrap();
        db.set_setting("ai_commit_message_enabled", "false")
            .unwrap();
        db.set_setting("ai_resolver_strategy", "smart_merge")
            .unwrap();
        db.set_setting("ai_commit_message_model", "claude-sonnet-4-20250514")
            .unwrap();

        // 2. "App startup" loads all settings (what dbGetAllSettings does)
        let all = db.get_all_settings();

        // 3. Apply each setting to app state (what use-app-state-init.ts does)
        assert_eq!(all.get("notification_sound_enabled"), Some(&"true".into()));
        assert_eq!(all.get("ai_commit_message_enabled"), Some(&"false".into()));
        assert_eq!(all.get("ai_resolver_strategy"), Some(&"smart_merge".into()));
        assert_eq!(
            all.get("ai_commit_message_model"),
            Some(&"claude-sonnet-4-20250514".into())
        );

        // 4. Individual get also works (for targeted reads)
        assert_eq!(
            db.get_setting("notification_sound_enabled"),
            Some("true".into())
        );

        // 5. Settings not in DB return None → app uses defaults
        assert_eq!(db.get_setting("theme_preset"), None);
    }

    #[test]
    fn wiring_workspace_state_roundtrip() {
        // Simulates: activate_workspace writes → SQLite → app restart reads
        let db = init_test_database();

        // 1. "activate_workspace" saves active workspace ID
        db.set_ui_state("active_workspace", "workspace-abc123")
            .unwrap();

        // 2. Also save collapse states for sidebar project groups
        db.set_ui_state("collapsed:project:/home/user/codemux", "true")
            .unwrap();
        db.set_ui_state("collapsed:project:/home/user/other", "false")
            .unwrap();

        // 3. Save window dimensions
        db.set_ui_state("window_width", "1920").unwrap();
        db.set_ui_state("window_height", "1080").unwrap();

        // 4. Save right panel width
        db.set_ui_state("right_panel_width", "320").unwrap();

        // 5. "App restart" reads everything back
        assert_eq!(
            db.get_ui_state("active_workspace"),
            Some("workspace-abc123".into())
        );
        assert_eq!(
            db.get_ui_state("collapsed:project:/home/user/codemux"),
            Some("true".into())
        );
        assert_eq!(
            db.get_ui_state("collapsed:project:/home/user/other"),
            Some("false".into())
        );
        assert_eq!(db.get_ui_state("window_width"), Some("1920".into()));
        assert_eq!(db.get_ui_state("window_height"), Some("1080".into()));
        assert_eq!(db.get_ui_state("right_panel_width"), Some("320".into()));

        // 6. Switch workspace → update
        db.set_ui_state("active_workspace", "workspace-def456")
            .unwrap();
        assert_eq!(
            db.get_ui_state("active_workspace"),
            Some("workspace-def456".into())
        );
    }

    #[test]
    fn corruption_resilience() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("corrupt_test.db");

        // Phase 1: Create DB and write data
        {
            let conn = open_connection(&db_path).unwrap();
            create_schema(&conn).unwrap();
            let db = DatabaseStore {
                conn: Mutex::new(conn),
            };
            db.set_setting("important", "data").unwrap();
            db.set_ui_state("window_width", "1920").unwrap();
        }

        // Phase 2: Corrupt the file (truncate to 0 bytes)
        {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&db_path)
                .unwrap();
            file.write_all(b"").unwrap();
        }

        // Phase 3: Attempt to reopen — should not panic
        // SQLite will see it's not a valid DB and either:
        // a) Open successfully but fail on schema operations
        // b) Return an error on open
        // Either way, new_in_memory() fallback should work
        let result = open_connection(&db_path);
        match result {
            Ok(conn) => {
                // Connection opened but DB is empty/corrupt
                // create_schema may fail or succeed (SQLite may recreate from scratch)
                let schema_result = create_schema(&conn);
                if schema_result.is_ok() {
                    // Fresh DB was created — old data is gone but app works
                    let db = DatabaseStore {
                        conn: Mutex::new(conn),
                    };
                    assert_eq!(db.get_setting("important"), None); // Data lost, but no crash
                    db.set_setting("new_key", "works").unwrap(); // Can still write
                    assert_eq!(db.get_setting("new_key"), Some("works".into()));
                }
                // If schema_result failed, that's also acceptable — init_database
                // would fall back to new_in_memory() in the real app
            }
            Err(_) => {
                // Connection failed — the app's init_database fallback handles this
                let fallback = DatabaseStore::new_in_memory();
                fallback.set_setting("recovery", "works").unwrap();
                assert_eq!(fallback.get_setting("recovery"), Some("works".into()));
            }
        }
    }

    #[test]
    fn large_data_values() {
        let db = init_test_database();

        // 10KB setting value (like serialized layout state)
        let large_json = format!(
            r#"{{"workspaces":[{}]}}"#,
            (0..100)
                .map(|i| format!(r#"{{"id":"ws-{i}","title":"Workspace {i}","cwd":"/home/user/project-{i}","tabs":[{{"id":"tab-{i}","kind":"terminal"}}]}}"#))
                .collect::<Vec<_>>()
                .join(",")
        );
        assert!(
            large_json.len() > 10_000,
            "Test value should be >10KB, got {} bytes",
            large_json.len()
        );

        db.set_setting("layout_state", &large_json).unwrap();
        let read_back = db.get_setting("layout_state");
        assert_eq!(read_back, Some(large_json.clone()));

        // 10KB ui_state value
        db.set_ui_state("scrollback_buffer", &large_json).unwrap();
        assert_eq!(
            db.get_ui_state("scrollback_buffer"),
            Some(large_json.clone())
        );

        // 50KB value
        let huge = "x".repeat(50_000);
        db.set_setting("comm_log", &huge).unwrap();
        assert_eq!(db.get_setting("comm_log"), Some(huge));

        // Multiple large values don't interfere
        let large_a = "a".repeat(10_000);
        let large_b = "b".repeat(10_000);
        db.set_setting("large_a", &large_a).unwrap();
        db.set_setting("large_b", &large_b).unwrap();
        assert_eq!(db.get_setting("large_a"), Some(large_a));
        assert_eq!(db.get_setting("large_b"), Some(large_b));
    }

    #[test]
    fn project_scripts_roundtrip() {
        let db = init_test_database();
        let scripts = ProjectScripts {
            setup: vec!["npm install".into(), "cp .env.example .env".into()],
            teardown: vec!["docker compose down".into()],
            run: Some("npm run dev".into()),
            ..Default::default()
        };
        db.set_project_scripts("/home/user/my-project", &scripts)
            .unwrap();
        let loaded = db
            .get_project_scripts("/home/user/my-project")
            .expect("scripts should exist");
        assert_eq!(loaded.setup, vec!["npm install", "cp .env.example .env"]);
        assert_eq!(loaded.teardown, vec!["docker compose down"]);
        assert_eq!(loaded.run, Some("npm run dev".into()));
    }

    #[test]
    fn project_scripts_missing_returns_none() {
        let db = init_test_database();
        assert!(db.get_project_scripts("/nonexistent").is_none());
    }

    #[test]
    fn project_scripts_update() {
        let db = init_test_database();
        let scripts = ProjectScripts {
            setup: vec!["npm install".into()],
            teardown: vec![],
            run: None,
            ..Default::default()
        };
        db.set_project_scripts("/project", &scripts).unwrap();

        let updated = ProjectScripts {
            setup: vec!["yarn install".into()],
            teardown: vec!["echo bye".into()],
            run: Some("yarn dev".into()),
            ..Default::default()
        };
        db.set_project_scripts("/project", &updated).unwrap();

        let loaded = db.get_project_scripts("/project").unwrap();
        assert_eq!(loaded.setup, vec!["yarn install"]);
        assert_eq!(loaded.teardown, vec!["echo bye"]);
        assert_eq!(loaded.run, Some("yarn dev".into()));
    }

    // ── Agent Chat Sessions ──

    #[test]
    fn agent_chat_sessions_upsert_and_fetch() {
        let db = init_test_database();
        db.upsert_agent_chat_session("thread-1", "ws-1", Some("/tmp/proj"), "claude")
        .unwrap();

        let rec = db
            .get_agent_chat_session("thread-1")
            .expect("row should exist");
        assert_eq!(rec.thread_id, "thread-1");
        assert_eq!(rec.workspace_id, "ws-1");
        assert_eq!(rec.cwd.as_deref(), Some("/tmp/proj"));
        assert_eq!(rec.provider, "claude");
        assert_eq!(rec.title, None);
        assert_eq!(rec.sdk_session_id, None);
    }

    // ── agent_chat_checkpoints (issue #80) ──

    fn sample_checkpoint(thread_id: &str) -> AgentChatCheckpointRecord {
        AgentChatCheckpointRecord {
            thread_id: thread_id.to_string(),
            workspace_id: "ws-1".to_string(),
            repo_path: "/tmp/repo".to_string(),
            ref_name: format!("refs/codemux/checkpoints/{thread_id}"),
            snapshot_commit: "a".repeat(40),
            head_commit: "b".repeat(40),
            branch: Some("main".to_string()),
            created_at: String::new(), // assigned by SQLite on insert
        }
    }

    #[test]
    fn agent_chat_checkpoint_upsert_and_fetch() {
        let db = init_test_database();
        db.upsert_agent_chat_session("t1", "ws-1", Some("/tmp/repo"), "claude")
            .unwrap();
        db.upsert_agent_chat_checkpoint(&sample_checkpoint("t1"))
            .unwrap();

        let rec = db.get_agent_chat_checkpoint("t1").expect("row exists");
        assert_eq!(rec.thread_id, "t1");
        assert_eq!(rec.workspace_id, "ws-1");
        assert_eq!(rec.repo_path, "/tmp/repo");
        assert_eq!(rec.ref_name, "refs/codemux/checkpoints/t1");
        assert_eq!(rec.snapshot_commit, "a".repeat(40));
        assert_eq!(rec.head_commit, "b".repeat(40));
        assert_eq!(rec.branch.as_deref(), Some("main"));
        assert!(!rec.created_at.is_empty(), "created_at assigned by SQLite");

        // Replace-on-conflict: a fresh checkpoint for the same thread
        // overwrites the previous one wholesale.
        let mut updated = sample_checkpoint("t1");
        updated.snapshot_commit = "c".repeat(40);
        updated.branch = None;
        db.upsert_agent_chat_checkpoint(&updated).unwrap();
        let rec = db.get_agent_chat_checkpoint("t1").unwrap();
        assert_eq!(rec.snapshot_commit, "c".repeat(40));
        assert_eq!(rec.branch, None);

        assert!(db.get_agent_chat_checkpoint("missing").is_none());
    }

    #[test]
    fn agent_chat_checkpoint_cascades_with_session_delete() {
        let db = init_test_database();
        db.upsert_agent_chat_session("t1", "ws-1", None, "claude")
            .unwrap();
        db.upsert_agent_chat_checkpoint(&sample_checkpoint("t1"))
            .unwrap();
        assert!(db.get_agent_chat_checkpoint("t1").is_some());

        db.delete_agent_chat_session("t1").unwrap();
        assert!(
            db.get_agent_chat_checkpoint("t1").is_none(),
            "checkpoint row should cascade-delete with the session"
        );
    }

    #[test]
    fn agent_chat_checkpoint_delete_by_pruned_refs_is_repo_scoped() {
        let db = init_test_database();
        for (thread, repo) in [("t1", "/repo/a"), ("t2", "/repo/b")] {
            db.upsert_agent_chat_session(thread, "ws-1", Some(repo), "claude")
                .unwrap();
            let mut cp = sample_checkpoint(thread);
            cp.repo_path = repo.to_string();
            // Same ref name in both repos — the prune delete must only
            // hit the matching repo's row.
            cp.ref_name = "refs/codemux/checkpoints/shared".to_string();
            db.upsert_agent_chat_checkpoint(&cp).unwrap();
        }

        db.delete_agent_chat_checkpoints_by_refs(
            "/repo/a",
            &["refs/codemux/checkpoints/shared".to_string()],
        )
        .unwrap();
        assert!(
            db.get_agent_chat_checkpoint("t1").is_none(),
            "pruned row gone"
        );
        assert!(
            db.get_agent_chat_checkpoint("t2").is_some(),
            "other repo's row untouched"
        );

        // Empty list is a no-op.
        db.delete_agent_chat_checkpoints_by_refs("/repo/b", &[])
            .unwrap();
        assert!(db.get_agent_chat_checkpoint("t2").is_some());
    }

    #[test]
    fn agent_chat_sessions_upsert_on_conflict_clears_cross_provider_cursor() {
        let db = init_test_database();
        db.upsert_agent_chat_session("t", "ws-1", Some("/a"), "claude")
            .unwrap();
        db.set_agent_chat_title("t", "Original title").unwrap();
        db.set_agent_chat_sdk_session_id("t", "sdk-uuid-xyz")
            .unwrap();

        // A same-provider restart keeps the provider-native cursor.
        db.upsert_agent_chat_session("t", "ws-1", Some("/a"), "claude")
            .unwrap();
        assert_eq!(
            db.get_agent_chat_session("t")
                .unwrap()
                .sdk_session_id
                .as_deref(),
            Some("sdk-uuid-xyz")
        );

        // Re-upsert with a different provider — the row and its
        // human-facing identity survive, but the Claude-native resume
        // cursor must not be handed to Codex.
        db.upsert_agent_chat_session("t", "ws-1", Some("/a"), "codex")
            .unwrap();

        let rec = db.get_agent_chat_session("t").unwrap();
        assert_eq!(rec.provider, "codex");
        assert_eq!(rec.title.as_deref(), Some("Original title"));
        assert_eq!(rec.sdk_session_id, None);
    }

    #[test]
    fn agent_chat_sessions_set_sdk_session_id() {
        let db = init_test_database();
        db.upsert_agent_chat_session("t", "ws-1", None, "claude")
            .unwrap();
        assert!(db
            .get_agent_chat_session("t")
            .unwrap()
            .sdk_session_id
            .is_none());

        db.set_agent_chat_sdk_session_id("t", "sdk-uuid-abc")
            .unwrap();
        assert_eq!(
            db.get_agent_chat_session("t")
                .unwrap()
                .sdk_session_id
                .as_deref(),
            Some("sdk-uuid-abc")
        );
    }

    #[test]
    fn agent_chat_sessions_clear_sdk_session_id() {
        let db = init_test_database();
        db.upsert_agent_chat_session("t", "ws-1", None, "claude")
            .unwrap();
        db.set_agent_chat_sdk_session_id("t", "sdk-uuid-abc")
            .unwrap();
        assert_eq!(
            db.get_agent_chat_session("t")
                .unwrap()
                .sdk_session_id
                .as_deref(),
            Some("sdk-uuid-abc")
        );

        // Stale-session recovery drops the dead id; the row survives.
        db.clear_agent_chat_sdk_session_id("t").unwrap();
        let rec = db.get_agent_chat_session("t").unwrap();
        assert_eq!(rec.sdk_session_id, None);

        // Idempotent — clearing an already-null / unknown row is a no-op.
        db.clear_agent_chat_sdk_session_id("t").unwrap();
        db.clear_agent_chat_sdk_session_id("no-such-thread")
            .unwrap();
    }

    #[test]
    fn agent_chat_sessions_title_roundtrip() {
        let db = init_test_database();
        db.upsert_agent_chat_session("t", "ws-1", None, "claude")
            .unwrap();
        assert_eq!(db.get_agent_chat_title("t"), None);
        db.set_agent_chat_title("t", "Refactor the auth layer")
            .unwrap();
        assert_eq!(
            db.get_agent_chat_title("t").as_deref(),
            Some("Refactor the auth layer")
        );
        // Updating overwrites.
        db.set_agent_chat_title("t", "Rename").unwrap();
        assert_eq!(db.get_agent_chat_title("t").as_deref(), Some("Rename"));
    }

    #[test]
    fn agent_chat_sessions_list_filters_by_workspace_and_cwd() {
        let db = init_test_database();
        db.upsert_agent_chat_session("t1", "ws-a", Some("/p1"), "claude")
            .unwrap();
        db.set_agent_chat_sdk_session_id("t1", "sdk-t1").unwrap();
        db.upsert_agent_chat_session("t2", "ws-a", Some("/p2"), "claude")
            .unwrap();
        db.set_agent_chat_sdk_session_id("t2", "sdk-t2").unwrap();
        db.upsert_agent_chat_session("t3", "ws-b", Some("/p1"), "claude")
            .unwrap();
        db.set_agent_chat_sdk_session_id("t3", "sdk-t3").unwrap();

        // Workspace-only scope returns every row in that workspace.
        let all_in_a = db.list_agent_chat_sessions("ws-a", None, 100);
        assert_eq!(all_in_a.len(), 2);

        // Workspace + cwd scope narrows further.
        let p1_in_a = db.list_agent_chat_sessions("ws-a", Some("/p1"), 100);
        assert_eq!(p1_in_a.len(), 1);
        assert_eq!(p1_in_a[0].thread_id, "t1");

        // Different workspace returns no crossover.
        let p1_in_b = db.list_agent_chat_sessions("ws-b", Some("/p1"), 100);
        assert_eq!(p1_in_b.len(), 1);
        assert_eq!(p1_in_b[0].thread_id, "t3");
    }

    #[test]
    fn agent_chat_sessions_list_hides_rows_without_sdk_session_id() {
        // Rows created by every start_session() but never graduated
        // to "have an SDK session id" are useless for resume — they
        // shouldn't appear in the dropdown. This is the main
        // practical cause of dropdown clutter: every silent restart
        // creates a fresh row and most never receive a
        // ResumeCursorUpdated event.
        let db = init_test_database();
        db.upsert_agent_chat_session("with-id", "ws", Some("/p"), "claude")
            .unwrap();
        db.set_agent_chat_sdk_session_id("with-id", "sdk-uuid")
            .unwrap();
        db.upsert_agent_chat_session("no-id", "ws", Some("/p"), "claude")
            .unwrap();

        let listed = db.list_agent_chat_sessions("ws", None, 100);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].thread_id, "with-id");

        let scoped = db.list_agent_chat_sessions("ws", Some("/p"), 100);
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].thread_id, "with-id");
    }

    #[test]
    fn collapse_duplicate_agent_chat_sessions_keeps_survivor() {
        // Simulate the resume flow creating multiple rows for the
        // same logical chat: each row shares an sdk_session_id but
        // has a fresh thread_id.
        let db = init_test_database();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO agent_chat_sessions
                     (thread_id, sdk_session_id, workspace_id, provider, title, created_at, last_active_at)
                 VALUES ('oldest', 'sdk-xyz', 'ws', 'claude', 'Original', '2025-01-01', '2025-01-01')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO agent_chat_sessions
                     (thread_id, sdk_session_id, workspace_id, provider, created_at, last_active_at)
                 VALUES ('middle', 'sdk-xyz', 'ws', 'claude', '2025-02-01', '2025-02-01')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO agent_chat_sessions
                     (thread_id, sdk_session_id, workspace_id, provider, created_at, last_active_at)
                 VALUES ('newest', 'sdk-xyz', 'ws', 'claude', '2025-03-01', '2025-03-01')",
                [],
            )
            .unwrap();
        }

        db.collapse_duplicate_agent_chat_sessions("sdk-xyz")
            .unwrap();

        // Only the newest row survives.
        assert!(db.get_agent_chat_session("oldest").is_none());
        assert!(db.get_agent_chat_session("middle").is_none());
        let survivor = db.get_agent_chat_session("newest").unwrap();
        // Title carried forward from the original row.
        assert_eq!(survivor.title.as_deref(), Some("Original"));
        // created_at carried forward to the earliest value.
        assert_eq!(survivor.created_at, "2025-01-01");
    }

    #[test]
    fn collapse_duplicate_agent_chat_sessions_carries_forward_config() {
        // Regression: reopening a session from the history dropdown
        // starts a fresh (most-recently-active) resume row with NULL
        // config, which becomes the survivor. The merge must carry the
        // original row's per-thread model / effort / context /
        // permission-mode onto the survivor before the original is
        // DELETEd, or the user's saved selection is silently lost.
        let db = init_test_database();
        {
            let conn = db.conn.lock().unwrap();
            // Original row (older) carries the user's real config.
            conn.execute(
                "INSERT INTO agent_chat_sessions
                     (thread_id, sdk_session_id, workspace_id, provider, created_at, last_active_at,
                      model, effort, context_window, permission_mode, fast_mode)
                 VALUES ('original', 'sdk-xyz', 'ws', 'claude', '2025-01-01', '2025-01-01',
                         'claude-opus-4-8', 'high', '1m', 'acceptEdits', 1)",
                [],
            )
            .unwrap();
            // Freshly-minted resume row (more recent) with NULL config.
            conn.execute(
                "INSERT INTO agent_chat_sessions
                     (thread_id, sdk_session_id, workspace_id, provider, created_at, last_active_at)
                 VALUES ('resume', 'sdk-xyz', 'ws', 'claude', '2025-02-01', '2025-02-01')",
                [],
            )
            .unwrap();
        }

        db.collapse_duplicate_agent_chat_sessions("sdk-xyz")
            .unwrap();

        // The resume row survives, but the original's config is
        // backfilled onto it.
        assert!(db.get_agent_chat_session("original").is_none());
        let survivor = db.get_agent_chat_session("resume").unwrap();
        assert_eq!(survivor.model.as_deref(), Some("claude-opus-4-8"));
        assert_eq!(survivor.effort.as_deref(), Some("high"));
        assert_eq!(survivor.context_window.as_deref(), Some("1m"));
        assert_eq!(survivor.permission_mode.as_deref(), Some("acceptEdits"));
        assert!(survivor.fast_mode);
    }

    #[test]
    fn collapse_duplicate_agent_chat_sessions_is_noop_when_no_match() {
        let db = init_test_database();
        db.upsert_agent_chat_session("t", "ws", None, "claude")
            .unwrap();
        // No sdk_session_id set — collapse must not touch the row.
        db.collapse_duplicate_agent_chat_sessions("sdk-missing")
            .unwrap();
        assert!(db.get_agent_chat_session("t").is_some());
    }

    #[test]
    fn collapse_duplicate_agent_chat_sessions_returns_external_cleanup() {
        let db = init_test_database();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO agent_chat_sessions
                     (thread_id, sdk_session_id, workspace_id, cwd, provider, created_at, last_active_at)
                 VALUES ('old', 'sdk-cleanup', 'ws', '/repo/cwd', 'codex', '2025-01-01', '2025-01-01')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO agent_chat_sessions
                     (thread_id, sdk_session_id, workspace_id, cwd, provider, created_at, last_active_at)
                 VALUES ('new', 'sdk-cleanup', 'ws', '/repo/new', 'codex', '2025-02-01', '2025-02-01')",
                [],
            )
            .unwrap();
        }
        db.upsert_agent_chat_turn_checkpoint(&sample_turn_checkpoint("old", 1, "nonce", 0))
            .unwrap();
        db.upsert_agent_chat_checkpoint(&AgentChatCheckpointRecord {
            thread_id: "old".to_string(),
            workspace_id: "ws".to_string(),
            repo_path: "/repo/legacy".to_string(),
            ref_name: "refs/codemux/checkpoints/old".to_string(),
            snapshot_commit: "legacy-snapshot".to_string(),
            head_commit: "head".to_string(),
            branch: Some("main".to_string()),
            created_at: String::new(),
        })
        .unwrap();

        let cleanup = db
            .collapse_duplicate_agent_chat_sessions("sdk-cleanup")
            .unwrap();

        assert_eq!(cleanup.thread_ids, vec!["old"]);
        assert_eq!(
            cleanup.repo_paths,
            vec![
                ("old".to_string(), "/repo/cwd".to_string()),
                ("old".to_string(), "/repo/legacy".to_string()),
                ("old".to_string(), "/tmp/checkpoint-repo".to_string()),
            ]
        );
        assert_eq!(
            cleanup.checkpoint_refs,
            vec![
                (
                    "/repo/legacy".to_string(),
                    "refs/codemux/checkpoints/old".to_string(),
                ),
                (
                    "/tmp/checkpoint-repo".to_string(),
                    "refs/codemux/turn-checkpoints/old/1".to_string(),
                ),
            ]
        );
        assert!(db.get_agent_chat_session("old").is_none());
        assert!(db.list_agent_chat_turn_checkpoints("old").is_empty());
        assert!(db.get_agent_chat_checkpoint("old").is_none());
        assert!(db.get_agent_chat_session("new").is_some());
    }

    #[test]
    fn agent_chat_sessions_list_orders_by_last_active_desc() {
        let db = init_test_database();
        // Seed with explicit timestamps so ordering is deterministic.
        // Each row needs a non-null sdk_session_id or list() filters
        // it out.
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO agent_chat_sessions
                     (thread_id, sdk_session_id, workspace_id, provider, created_at, last_active_at)
                 VALUES ('old', 'sdk-old', 'ws', 'claude', '2025-01-01', '2025-01-01')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO agent_chat_sessions
                     (thread_id, sdk_session_id, workspace_id, provider, created_at, last_active_at)
                 VALUES ('newest', 'sdk-newest', 'ws', 'claude', '2025-01-01', '2025-06-01')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO agent_chat_sessions
                     (thread_id, sdk_session_id, workspace_id, provider, created_at, last_active_at)
                 VALUES ('mid', 'sdk-mid', 'ws', 'claude', '2025-01-01', '2025-03-01')",
                [],
            )
            .unwrap();
        }
        let rows = db.list_agent_chat_sessions("ws", None, 10);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].thread_id, "newest");
        assert_eq!(rows[1].thread_id, "mid");
        assert_eq!(rows[2].thread_id, "old");
    }

    #[test]
    fn agent_chat_sessions_touch_floats_to_top() {
        let db = init_test_database();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO agent_chat_sessions
                     (thread_id, sdk_session_id, workspace_id, provider, created_at, last_active_at)
                 VALUES ('a', 'sdk-a', 'ws', 'claude', '2025-01-01', '2025-01-01')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO agent_chat_sessions
                     (thread_id, sdk_session_id, workspace_id, provider, created_at, last_active_at)
                 VALUES ('b', 'sdk-b', 'ws', 'claude', '2025-01-01', '2025-06-01')",
                [],
            )
            .unwrap();
        }
        // Before touch, 'b' is on top.
        let before = db.list_agent_chat_sessions("ws", None, 10);
        assert_eq!(before[0].thread_id, "b");

        // After touch on 'a', 'a' has today's timestamp → top.
        db.touch_agent_chat_session("a").unwrap();
        let after = db.list_agent_chat_sessions("ws", None, 10);
        assert_eq!(after[0].thread_id, "a");
    }

    #[test]
    fn agent_chat_sessions_delete_is_idempotent() {
        let db = init_test_database();
        db.upsert_agent_chat_session("t", "ws", None, "claude")
            .unwrap();
        assert!(db.get_agent_chat_session("t").is_some());
        db.delete_agent_chat_session("t").unwrap();
        assert!(db.get_agent_chat_session("t").is_none());
        // Second call is a no-op.
        db.delete_agent_chat_session("t").unwrap();
    }

    #[test]
    fn agent_chat_sessions_migrate_carries_forward_identity() {
        let db = init_test_database();
        db.upsert_agent_chat_session("old", "ws", Some("/p"), "claude")
            .unwrap();
        db.set_agent_chat_title("old", "My chat").unwrap();
        db.set_agent_chat_sdk_session_id("old", "sdk-uuid").unwrap();

        // Simulate migrateThreadId: caller upserts the new row first,
        // then asks us to carry forward the identity fields.
        db.upsert_agent_chat_session("new", "ws", Some("/p"), "claude")
            .unwrap();
        db.migrate_agent_chat_session("old", "new").unwrap();

        assert!(db.get_agent_chat_session("old").is_none());
        let rec = db.get_agent_chat_session("new").unwrap();
        assert_eq!(rec.title.as_deref(), Some("My chat"));
        assert_eq!(rec.sdk_session_id.as_deref(), Some("sdk-uuid"));
    }

    // ── Per-thread chat config (restart-resume follow-up) ──

    #[test]
    fn agent_chat_session_config_defaults_null_on_fresh_row() {
        let db = init_test_database();
        db.upsert_agent_chat_session("t", "ws", Some("/p"), "claude")
            .unwrap();
        let rec = db.get_agent_chat_session("t").expect("row exists");
        // A brand-new row has no nullable config yet and uses Standard speed.
        assert_eq!(rec.model, None);
        assert_eq!(rec.effort, None);
        assert_eq!(rec.context_window, None);
        assert_eq!(rec.permission_mode, None);
        assert!(!rec.fast_mode);
    }

    #[test]
    fn agent_chat_session_config_roundtrip_and_get_null_sdk_row() {
        let db = init_test_database();
        // No sdk_session_id set — this is exactly the post-restart state
        // `agent_chat_get_session` must still return (unlike the history
        // list, which filters NULL-sdk rows out).
        db.upsert_agent_chat_session("t", "ws", Some("/p"), "claude")
            .unwrap();
        db.update_agent_chat_session_config(
            "t",
            &AgentChatSessionConfig {
                model: AgentChatSessionConfig::set("claude-opus-4-8"),
                effort: AgentChatSessionConfig::set("high"),
                context_window: AgentChatSessionConfig::set("1m"),
                permission_mode: AgentChatSessionConfig::set("acceptEdits"),
                fast_mode: Some(true),
            },
        )
        .unwrap();

        let rec = db
            .get_agent_chat_session("t")
            .expect("get returns the row even with a NULL sdk_session_id");
        assert_eq!(rec.sdk_session_id, None);
        assert_eq!(rec.model.as_deref(), Some("claude-opus-4-8"));
        assert_eq!(rec.effort.as_deref(), Some("high"));
        assert_eq!(rec.context_window.as_deref(), Some("1m"));
        assert_eq!(rec.permission_mode.as_deref(), Some("acceptEdits"));
        assert!(rec.fast_mode);
    }

    #[test]
    fn agent_chat_update_session_config_only_overwrites_provided_fields() {
        let db = init_test_database();
        db.upsert_agent_chat_session("t", "ws", Some("/p"), "claude")
            .unwrap();
        db.update_agent_chat_session_config(
            "t",
            &AgentChatSessionConfig {
                model: AgentChatSessionConfig::set("claude-opus-4-8"),
                effort: AgentChatSessionConfig::set("high"),
                context_window: AgentChatSessionConfig::set("1m"),
                permission_mode: AgentChatSessionConfig::set("default"),
                fast_mode: Some(true),
            },
        )
        .unwrap();

        // A partial update (only model) must leave the other columns
        // untouched — an absent (`None`) field is omitted from the UPDATE.
        db.update_agent_chat_session_config(
            "t",
            &AgentChatSessionConfig {
                model: AgentChatSessionConfig::set("claude-sonnet-4-5"),
                ..AgentChatSessionConfig::default()
            },
        )
        .unwrap();

        let rec = db.get_agent_chat_session("t").unwrap();
        assert_eq!(rec.model.as_deref(), Some("claude-sonnet-4-5"));
        assert_eq!(rec.effort.as_deref(), Some("high"), "effort preserved");
        assert_eq!(
            rec.context_window.as_deref(),
            Some("1m"),
            "context_window preserved"
        );
        assert_eq!(
            rec.permission_mode.as_deref(),
            Some("default"),
            "permission_mode preserved"
        );
        assert!(rec.fast_mode, "fast_mode preserved");
    }

    #[test]
    fn agent_chat_update_session_config_explicit_null_clears_column() {
        // Regression: the model-change compat reset sends an explicit
        // `null` for effort / context_window when the new model no longer
        // supports them. That must CLEAR the column (`Some(None)`), not be
        // dropped as "leave untouched" — otherwise a model-incompatible
        // effort/context survives a restart and gets re-applied.
        let db = init_test_database();
        db.upsert_agent_chat_session("t", "ws", Some("/p"), "claude")
            .unwrap();
        db.update_agent_chat_session_config(
            "t",
            &AgentChatSessionConfig {
                model: AgentChatSessionConfig::set("claude-opus-4-8"),
                effort: AgentChatSessionConfig::set("high"),
                context_window: AgentChatSessionConfig::set("1m"),
                ..AgentChatSessionConfig::default()
            },
        )
        .unwrap();

        // Switch to a model with no effort levels / no 1m context: the
        // patch keeps `model`, but clears effort + context to NULL while
        // leaving permission_mode (absent) untouched.
        db.update_agent_chat_session_config(
            "t",
            &AgentChatSessionConfig {
                model: AgentChatSessionConfig::set("claude-haiku-4-5"),
                effort: Some(None),
                context_window: Some(None),
                permission_mode: None,
                fast_mode: None,
            },
        )
        .unwrap();

        let rec = db.get_agent_chat_session("t").unwrap();
        assert_eq!(rec.model.as_deref(), Some("claude-haiku-4-5"));
        assert_eq!(rec.effort, None, "explicit null cleared effort");
        assert_eq!(
            rec.context_window, None,
            "explicit null cleared context_window"
        );
    }

    #[test]
    fn agent_chat_update_session_config_null_deserializes_to_clear() {
        // The tri-state must survive the JSON wire: an explicit `null`
        // from the frontend patch deserializes to `Some(None)` (clear),
        // while an absent field stays `None` (leave untouched).
        let cfg: AgentChatSessionConfig = serde_json::from_str(
            r#"{ "model": "claude-haiku-4-5", "effort": null, "context_window": null }"#,
        )
        .unwrap();
        assert_eq!(cfg.model, Some(Some("claude-haiku-4-5".to_string())));
        assert_eq!(cfg.effort, Some(None), "explicit null → clear");
        assert_eq!(cfg.context_window, Some(None), "explicit null → clear");
        assert_eq!(cfg.permission_mode, None, "absent field → untouched");
    }

    #[test]
    fn upsert_preserves_config_on_conflict() {
        let db = init_test_database();
        db.upsert_agent_chat_session("t", "ws", Some("/p"), "claude")
            .unwrap();
        db.update_agent_chat_session_config(
            "t",
            &AgentChatSessionConfig {
                model: AgentChatSessionConfig::set("claude-opus-4-8"),
                ..AgentChatSessionConfig::default()
            },
        )
        .unwrap();
        // A later upsert (e.g. a silent restart bumping last_active_at)
        // must not clobber the persisted config.
        db.upsert_agent_chat_session("t", "ws", Some("/p2"), "claude")
            .unwrap();
        let rec = db.get_agent_chat_session("t").unwrap();
        assert_eq!(rec.cwd.as_deref(), Some("/p2"), "cwd refreshed by upsert");
        assert_eq!(
            rec.model.as_deref(),
            Some("claude-opus-4-8"),
            "config survives the upsert"
        );
    }

    #[test]
    fn agent_chat_sessions_migrate_carries_forward_config() {
        let db = init_test_database();
        db.upsert_agent_chat_session("old", "ws", Some("/p"), "claude")
            .unwrap();
        db.update_agent_chat_session_config(
            "old",
            &AgentChatSessionConfig {
                model: AgentChatSessionConfig::set("claude-opus-4-8"),
                effort: AgentChatSessionConfig::set("xhigh"),
                context_window: AgentChatSessionConfig::set("1m"),
                permission_mode: AgentChatSessionConfig::set("bypassPermissions"),
                fast_mode: Some(true),
            },
        )
        .unwrap();

        db.upsert_agent_chat_session("new", "ws", Some("/p"), "claude")
            .unwrap();
        db.migrate_agent_chat_session("old", "new").unwrap();

        let rec = db.get_agent_chat_session("new").expect("migrated row");
        assert_eq!(rec.model.as_deref(), Some("claude-opus-4-8"));
        assert_eq!(rec.effort.as_deref(), Some("xhigh"));
        assert_eq!(rec.context_window.as_deref(), Some("1m"));
        assert_eq!(rec.permission_mode.as_deref(), Some("bypassPermissions"));
        assert!(rec.fast_mode);
    }

    #[test]
    fn list_agent_chat_sessions_returns_config_columns() {
        let db = init_test_database();
        db.upsert_agent_chat_session("t", "ws", Some("/p"), "claude")
            .unwrap();
        db.set_agent_chat_sdk_session_id("t", "sdk-1").unwrap();
        db.update_agent_chat_session_config(
            "t",
            &AgentChatSessionConfig {
                model: AgentChatSessionConfig::set("claude-opus-4-8"),
                effort: AgentChatSessionConfig::set("high"),
                fast_mode: Some(true),
                ..AgentChatSessionConfig::default()
            },
        )
        .unwrap();
        let rows = db.list_agent_chat_sessions("ws", None, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].model.as_deref(), Some("claude-opus-4-8"));
        assert_eq!(rows[0].effort.as_deref(), Some("high"));
        assert!(rows[0].fast_mode);
    }

    #[test]
    fn update_config_on_missing_row_is_noop() {
        let db = init_test_database();
        // No row for "ghost" — the UPDATE matches zero rows and returns Ok.
        db.update_agent_chat_session_config(
            "ghost",
            &AgentChatSessionConfig {
                model: AgentChatSessionConfig::set("claude-opus-4-8"),
                ..AgentChatSessionConfig::default()
            },
        )
        .expect("update on missing row is a no-op, not an error");
        assert!(db.get_agent_chat_session("ghost").is_none());
    }

    #[test]
    fn agent_chat_sessions_limit_caps_rows() {
        let db = init_test_database();
        for i in 0..10 {
            db.upsert_agent_chat_session(&format!("t{i}"), "ws", None, "claude")
            .unwrap();
            db.set_agent_chat_sdk_session_id(&format!("t{i}"), &format!("sdk-{i}"))
                .unwrap();
        }
        let capped = db.list_agent_chat_sessions("ws", None, 3);
        assert_eq!(capped.len(), 3);
    }

    // ── Agent Chat Messages ──

    fn seed_session(db: &DatabaseStore, thread_id: &str) {
        db.upsert_agent_chat_session(thread_id, "ws", Some("/p"), "claude")
            .unwrap();
    }

    #[test]
    fn session_mentions_are_workspace_scoped_safe_and_checkout_first() {
        let db = init_test_database();
        seed_session(&db, "current");
        db.set_agent_chat_title("current", "Current checkout work")
            .unwrap();
        db.append_agent_chat_message(
            "current",
            r#"{"type":"user_message","text":"opening task"}"#,
        )
        .unwrap();
        db.append_agent_chat_message(
            "current",
            r#"{"type":"item_completed","item":{"kind":"tool_result","content":"private tool bytes"}}"#,
        )
        .unwrap();
        db.append_agent_chat_message(
            "current",
            r#"{"type":"item_completed","item":{"kind":"assistant_text","text":"latest safe progress"}}"#,
        )
        .unwrap();

        db.upsert_agent_chat_session("other-cwd", "ws", Some("/worktree"), "codex")
            .unwrap();
        db.append_agent_chat_message(
            "other-cwd",
            r#"{"type":"user_message","text":"worktree task"}"#,
        )
        .unwrap();
        db.upsert_agent_chat_session("outside", "other", Some("/p"), "claude")
            .unwrap();
        db.append_agent_chat_message(
            "outside",
            r#"{"type":"user_message","text":"must stay outside"}"#,
        )
        .unwrap();

        let rows = db
            .list_agent_chat_session_mentions("ws", Some("/p"), None, 20)
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].thread_id, "current");
        assert_eq!(rows[0].preview, "latest safe progress");
        assert_eq!(rows[0].message_count, 2, "tool output is not indexed");
        assert_eq!(rows[1].thread_id, "other-cwd");

        let excluded = db
            .list_agent_chat_session_mentions("ws", Some("/p"), Some("current"), 20)
            .unwrap();
        assert_eq!(excluded.len(), 1);
        assert_eq!(excluded[0].thread_id, "other-cwd");

        let visible = db.list_agent_chat_visible_messages("current").unwrap();
        assert_eq!(
            visible,
            vec![
                AgentChatVisibleMessage {
                    role: "user".into(),
                    content: "opening task".into(),
                },
                AgentChatVisibleMessage {
                    role: "assistant".into(),
                    content: "latest safe progress".into(),
                },
            ],
        );
    }

    #[test]
    fn agent_chat_search_indexes_only_visible_conversation_prose() {
        let db = init_test_database();
        seed_session(&db, "t");
        db.set_agent_chat_title("t", "Release investigation")
            .unwrap();

        let user_id = db
            .append_agent_chat_message(
                "t",
                r#"{"type":"user_message","text":"Investigate deployment latency"}"#,
            )
            .unwrap()
            .unwrap();
        let assistant_id = db
            .append_agent_chat_message(
                "t",
                r#"{"type":"item_completed","thread_id":"t","turn_id":"turn-7","item":{"kind":"assistant_text","text":"The deployment bottleneck is the cache warmup."}}"#,
            )
            .unwrap()
            .unwrap();
        db.append_agent_chat_message(
            "t",
            r#"{"type":"item_completed","thread_id":"t","turn_id":"turn-7","item":{"kind":"tool_result","content":"toolbodysecret deployment"}}"#,
        )
        .unwrap();
        db.append_agent_chat_message(
            "t",
            r#"{"type":"item_completed","thread_id":"t","turn_id":"turn-7","subagent_id":"sub-1","item":{"kind":"assistant_text","text":"subagentsecret deployment"}}"#,
        )
        .unwrap();
        db.append_agent_chat_message(
            "t",
            r#"{"type":"item_completed","thread_id":"t","turn_id":"turn-7","item":{"kind":"assistant_thinking","text":"thinkingsecret deployment"}}"#,
        )
        .unwrap();

        let hits = db
            .search_agent_chat("deploy", &["ws".to_string()], 20)
            .unwrap();
        assert_eq!(hits.len(), 2, "user and top-level assistant text only");
        assert!(hits.iter().any(|hit| {
            hit.role == "user" && hit.message_id == Some(user_id) && hit.turn_id.is_none()
        }));
        assert!(hits.iter().any(|hit| {
            hit.role == "assistant"
                && hit.message_id == Some(assistant_id)
                && hit.turn_id.as_deref() == Some("turn-7")
        }));
        for excluded in ["toolbodysecret", "subagentsecret", "thinkingsecret"] {
            assert!(db
                .search_agent_chat(excluded, &["ws".to_string()], 20)
                .unwrap()
                .is_empty());
        }
    }

    #[test]
    fn conversation_history_tools_page_search_and_enforce_workspace_scope() {
        let db = init_test_database();
        seed_session(&db, "source");
        let first = db
            .append_agent_chat_message(
                "source",
                r#"{"type":"user_message","text":"Investigate the azure timeout"}"#,
            )
            .unwrap()
            .unwrap();
        db.append_agent_chat_message(
            "source",
            r#"{"type":"item_completed","turn_id":"turn-1","item":{"kind":"tool_result","content":"azure-tool-secret"}}"#,
        )
        .unwrap();
        let second = db
            .append_agent_chat_message(
                "source",
                r#"{"type":"item_completed","turn_id":"turn-1","item":{"kind":"assistant_text","text":"The retry now uses bounded backoff."}}"#,
            )
            .unwrap()
            .unwrap();
        let third = db
            .append_agent_chat_message(
                "source",
                r#"{"type":"user_message","text":"Verify the timeout test"}"#,
            )
            .unwrap()
            .unwrap();

        let first_page = db
            .read_agent_chat_history_page("ws", "source", None, 2)
            .unwrap();
        assert_eq!(first_page.total_visible_messages, 3);
        assert_eq!(
            first_page
                .messages
                .iter()
                .map(|message| message.message_id)
                .collect::<Vec<_>>(),
            vec![first, second]
        );
        assert_eq!(first_page.next_cursor, Some(second));

        let second_page = db
            .read_agent_chat_history_page("ws", "source", first_page.next_cursor, 2)
            .unwrap();
        assert_eq!(second_page.messages.len(), 1);
        assert_eq!(second_page.messages[0].message_id, third);
        assert_eq!(second_page.next_cursor, None);

        let hits = db
            .search_agent_chat_history("ws", "source", "timeout", 10)
            .unwrap();
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|hit| hit.snippet.contains("timeout")));
        assert!(db
            .search_agent_chat_history("ws", "source", "azure-tool-secret", 10)
            .unwrap()
            .is_empty());

        assert_eq!(
            db.read_agent_chat_history_page("other", "source", None, 10)
                .unwrap_err(),
            "conversation_outside_workspace"
        );
        assert_eq!(
            db.search_agent_chat_history("other", "source", "timeout", 10)
                .unwrap_err(),
            "conversation_outside_workspace"
        );
    }

    #[test]
    fn handoff_summary_cache_is_revisioned_replaceable_and_cascades() {
        let db = init_test_database();
        seed_session(&db, "source");
        let revision = db
            .append_agent_chat_message(
                "source",
                r#"{"type":"user_message","text":"Build a handoff"}"#,
            )
            .unwrap()
            .unwrap();
        assert_eq!(db.agent_chat_visible_revision("source").unwrap(), revision);

        db.put_agent_chat_handoff_summary(
            "source",
            revision,
            "codex",
            "gpt-5.6-luna",
            Some("low"),
            1,
            "## Goal\nBuild a handoff",
        )
        .unwrap();
        let cached = db
            .get_agent_chat_handoff_summary("source")
            .unwrap()
            .unwrap();
        assert_eq!(cached.revision_message_id, revision);
        assert_eq!(cached.summarizer_model, "gpt-5.6-luna");

        db.put_agent_chat_handoff_summary(
            "source",
            revision,
            "claude",
            "claude-haiku-4-5",
            None,
            2,
            "## Goal\nUpdated",
        )
        .unwrap();
        let replaced = db
            .get_agent_chat_handoff_summary("source")
            .unwrap()
            .unwrap();
        assert_eq!(replaced.summarizer_provider, "claude");
        assert_eq!(replaced.prompt_version, 2);
        assert_eq!(replaced.summary, "## Goal\nUpdated");

        db.delete_agent_chat_session("source").unwrap();
        assert!(db
            .get_agent_chat_handoff_summary("source")
            .unwrap()
            .is_none());
    }

    #[test]
    fn agent_chat_search_includes_titles_and_respects_workspace_scope() {
        let db = init_test_database();
        seed_session(&db, "inside");
        db.set_agent_chat_title("inside", "Virtualized transcript fix")
            .unwrap();
        db.upsert_agent_chat_session("outside", "other-ws", None, "codex")
            .unwrap();
        db.set_agent_chat_title("outside", "Virtualized transcript elsewhere")
            .unwrap();

        let hits = db
            .search_agent_chat("transcript", &["ws".to_string()], 10)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].thread_id, "inside");
        assert_eq!(hits[0].role, "title");
        assert_eq!(hits[0].message_id, None);
        assert_eq!(hits[0].workspace_id, "ws");
        assert!(db
            .search_agent_chat("transcript", &[], 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn agent_chat_search_index_tracks_migration_deletion_and_backfill() {
        let db = init_test_database();
        seed_session(&db, "old");
        let message_id = db
            .append_agent_chat_message(
                "old",
                r#"{"type":"user_message","text":"durable migration marker"}"#,
            )
            .unwrap()
            .unwrap();
        db.upsert_agent_chat_session("new", "ws", Some("/p"), "claude")
            .unwrap();
        db.migrate_agent_chat_session("old", "new").unwrap();

        let migrated = db
            .search_agent_chat("migration", &["ws".to_string()], 10)
            .unwrap();
        assert_eq!(migrated.len(), 1);
        assert_eq!(migrated[0].thread_id, "new");
        assert_eq!(migrated[0].message_id, Some(message_id));

        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "DELETE FROM agent_chat_search WHERE rowid = ?1",
                params![message_id],
            )
            .unwrap();
            conn.execute("UPDATE schema_version SET version = 13", [])
                .unwrap();
            create_schema(&conn).unwrap();
        }
        assert_eq!(
            db.search_agent_chat("migration", &["ws".to_string()], 10)
                .unwrap()
                .len(),
            1,
            "schema startup backfills a missing index row"
        );

        db.delete_agent_chat_session("new").unwrap();
        assert!(db
            .search_agent_chat("migration", &["ws".to_string()], 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn agent_chat_messages_append_and_list_preserves_insertion_order() {
        let db = init_test_database();
        seed_session(&db, "t");

        for i in 0..5 {
            db.append_agent_chat_message("t", &format!(r#"{{"i":{i}}}"#))
                .unwrap();
        }

        let rows = db.list_agent_chat_messages("t");
        assert_eq!(rows.len(), 5);
        for (i, row) in rows.iter().enumerate() {
            assert_eq!(row, &format!(r#"{{"i":{i}}}"#));
        }
    }

    #[test]
    fn agent_chat_messages_append_returns_monotonic_row_ids() {
        let db = init_test_database();
        seed_session(&db, "t");

        let first = db.append_agent_chat_message("t", r#"{"i":0}"#).unwrap();
        let second = db.append_agent_chat_message("t", r#"{"i":1}"#).unwrap();
        assert!(first.is_some() && second.is_some());
        assert!(second.unwrap() > first.unwrap());
        // The swallowed FK case wrote nothing, so it has no position.
        assert_eq!(
            db.append_agent_chat_message("ghost", r#"{}"#).unwrap(),
            None
        );
    }

    #[test]
    fn agent_chat_messages_after_filters_by_cursor_and_keeps_order() {
        let db = init_test_database();
        seed_session(&db, "t");
        let mut ids = Vec::new();
        for i in 0..5 {
            ids.push(
                db.append_agent_chat_message("t", &format!(r#"{{"i":{i}}}"#))
                    .unwrap()
                    .unwrap(),
            );
        }

        // `None` == from the beginning.
        let all = db.list_agent_chat_messages_after("t", None);
        assert_eq!(all.len(), 5);
        assert_eq!(all[0].0, ids[0]);
        assert_eq!(all[4].1, r#"{"i":4}"#);
        assert!(all[0].2 > 1_700_000_000_000);
        assert!(all.windows(2).all(|w| w[0].0 < w[1].0));

        // A cursor is exclusive.
        let tail = db.list_agent_chat_messages_after("t", Some(ids[2]));
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[0].1, r#"{"i":3}"#);

        // Warm revisit with nothing new.
        assert!(db
            .list_agent_chat_messages_after("t", Some(ids[4]))
            .is_empty());
        // A cursor above the head reads as "nothing new" — the frontend
        // catches that case with `max_agent_chat_message_id`.
        assert!(db
            .list_agent_chat_messages_after("t", Some(ids[4] + 1000))
            .is_empty());
    }

    #[test]
    fn agent_chat_messages_after_filters_by_thread() {
        let db = init_test_database();
        seed_session(&db, "t1");
        seed_session(&db, "t2");
        db.append_agent_chat_message("t1", r#"{"a":1}"#).unwrap();
        db.append_agent_chat_message("t2", r#"{"b":2}"#).unwrap();
        db.append_agent_chat_message("t1", r#"{"c":3}"#).unwrap();

        let t1 = db.list_agent_chat_messages_after("t1", None);
        assert_eq!(t1.len(), 2);
        assert_eq!(t1[1].1, r#"{"c":3}"#);
        assert_eq!(db.list_agent_chat_messages_after("unknown", None).len(), 0);
    }

    #[test]
    fn agent_chat_message_head_and_by_id_reads() {
        let db = init_test_database();
        seed_session(&db, "t");
        assert_eq!(db.max_agent_chat_message_id("t"), None);

        let first = db
            .append_agent_chat_message("t", r#"{"i":0}"#)
            .unwrap()
            .unwrap();
        let second = db
            .append_agent_chat_message("t", r#"{"i":1}"#)
            .unwrap()
            .unwrap();
        assert_eq!(db.max_agent_chat_message_id("t"), Some(second));
        assert_eq!(db.max_agent_chat_message_id("nope"), None);

        assert_eq!(
            db.get_agent_chat_message(first).as_deref(),
            Some(r#"{"i":0}"#)
        );
        assert_eq!(db.get_agent_chat_message(second + 999), None);
    }

    #[test]
    fn agent_chat_messages_list_returns_empty_for_unknown_thread() {
        let db = init_test_database();
        assert!(db.list_agent_chat_messages("nope").is_empty());
    }

    #[test]
    fn agent_chat_messages_list_filters_by_thread() {
        let db = init_test_database();
        seed_session(&db, "t1");
        seed_session(&db, "t2");

        db.append_agent_chat_message("t1", r#"{"a":1}"#).unwrap();
        db.append_agent_chat_message("t2", r#"{"b":2}"#).unwrap();
        db.append_agent_chat_message("t1", r#"{"c":3}"#).unwrap();

        let t1 = db.list_agent_chat_messages("t1");
        assert_eq!(t1, vec![r#"{"a":1}"#.to_string(), r#"{"c":3}"#.to_string()]);
        let t2 = db.list_agent_chat_messages("t2");
        assert_eq!(t2, vec![r#"{"b":2}"#.to_string()]);
    }

    #[test]
    fn agent_chat_messages_cascade_delete_on_session_drop() {
        // Cleanup hygiene: deleting a session row from the dropdown
        // also drops its persisted transcript, freeing disk and
        // hiding the chat from any "leak" of orphan rows.
        let db = init_test_database();
        seed_session(&db, "t");
        db.append_agent_chat_message("t", r#"{"x":1}"#).unwrap();
        db.append_agent_chat_message("t", r#"{"x":2}"#).unwrap();
        assert_eq!(db.list_agent_chat_messages("t").len(), 2);

        db.delete_agent_chat_session("t").unwrap();
        assert!(db.list_agent_chat_messages("t").is_empty());
    }

    #[test]
    fn agent_chat_messages_append_to_unknown_thread_is_silent_noop() {
        // FK violation: parent session row absent. The contract is
        // "best-effort persistence" — the event-bridge loop must not
        // crash on a race where forward_event fires after the user
        // has deleted the session.
        let db = init_test_database();
        let result = db.append_agent_chat_message("ghost", r#"{"x":1}"#);
        assert!(result.is_ok(), "FK violation should be swallowed silently");
        assert!(db.list_agent_chat_messages("ghost").is_empty());
    }

    #[test]
    fn agent_chat_messages_append_surfaces_unrelated_errors() {
        // Closed-on-purpose: only FK_CONSTRAINT is swallowed. Other
        // SQL errors must propagate so we don't silently accumulate
        // data corruption. Without an easy way to force a non-FK
        // failure in-memory, this test acts as documentation: if this
        // assert ever changes shape, revisit `append_agent_chat_message`.
        let db = init_test_database();
        seed_session(&db, "t");
        // Sanity: the happy path still returns Ok on a real row.
        assert!(db.append_agent_chat_message("t", r#"{"ok":true}"#).is_ok());
    }

    #[test]
    fn migrate_agent_chat_session_moves_messages_to_new_thread() {
        // Silent restart path: the user changes permission_mode mid-
        // conversation. The store-side migrateThreadId carries the
        // transcript forward in memory; the DB-side migration must
        // do the same, otherwise the next list_messages call sees an
        // empty history under the new thread id.
        let db = init_test_database();
        seed_session(&db, "old");
        db.append_agent_chat_message("old", r#"{"i":1}"#).unwrap();
        db.append_agent_chat_message("old", r#"{"i":2}"#).unwrap();

        seed_session(&db, "new");
        db.migrate_agent_chat_session("old", "new").unwrap();

        // Messages now live under "new", and the source row is gone.
        assert!(db.list_agent_chat_messages("old").is_empty());
        let migrated = db.list_agent_chat_messages("new");
        assert_eq!(
            migrated,
            vec![r#"{"i":1}"#.to_string(), r#"{"i":2}"#.to_string()]
        );
        assert!(db.get_agent_chat_session("old").is_none());
    }

    #[test]
    fn collapse_duplicate_agent_chat_sessions_moves_messages_onto_survivor() {
        // The dropdown's "Resume" path produces a new thread_id
        // sharing the original's sdk_session_id. Without the
        // pre-collapse UPDATE on agent_chat_messages, the FK cascade
        // would wipe the historical transcript when the duplicate
        // session row gets dropped — exactly the regression the
        // resume feature is meant to avoid.
        let db = init_test_database();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO agent_chat_sessions
                     (thread_id, sdk_session_id, workspace_id, provider, created_at, last_active_at)
                 VALUES ('oldest', 'sdk-xyz', 'ws', 'claude', '2025-01-01', '2025-01-01')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO agent_chat_sessions
                     (thread_id, sdk_session_id, workspace_id, provider, created_at, last_active_at)
                 VALUES ('newest', 'sdk-xyz', 'ws', 'claude', '2025-03-01', '2025-03-01')",
                [],
            )
            .unwrap();
        }
        db.append_agent_chat_message("oldest", r#"{"i":"old1"}"#)
            .unwrap();
        db.append_agent_chat_message("oldest", r#"{"i":"old2"}"#)
            .unwrap();
        db.append_agent_chat_message("newest", r#"{"i":"new1"}"#)
            .unwrap();

        db.collapse_duplicate_agent_chat_sessions("sdk-xyz")
            .unwrap();

        // Survivor "newest" now owns every message; the merged-out
        // row is gone.
        assert!(db.get_agent_chat_session("oldest").is_none());
        assert!(db.list_agent_chat_messages("oldest").is_empty());
        let survivor = db.list_agent_chat_messages("newest");
        // Order across the moved-in rows preserves the insertion id;
        // we don't test exact ordering here (it can interleave by id)
        // but verify count + every payload is present.
        assert_eq!(survivor.len(), 3);
        let joined = survivor.join(",");
        assert!(joined.contains(r#"{"i":"old1"}"#));
        assert!(joined.contains(r#"{"i":"old2"}"#));
        assert!(joined.contains(r#"{"i":"new1"}"#));
    }

    #[test]
    fn agent_chat_messages_supports_large_payloads() {
        // Tool results can carry serialized file contents — ~50KB
        // is plausible. SQLite handles arbitrary TEXT lengths but
        // make sure no hidden truncation crept in via column types.
        let db = init_test_database();
        seed_session(&db, "t");
        let big = format!(r#"{{"data":"{}"}}"#, "x".repeat(50_000));
        db.append_agent_chat_message("t", &big).unwrap();
        let rows = db.list_agent_chat_messages("t");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], big);
    }

    #[test]
    fn agent_chat_messages_high_volume_round_trip() {
        // Sanity: a long conversation (5 turns × ~10 events per
        // turn) round-trips cleanly. Catches any per-row overhead
        // regressions or query-prepare bugs that might surface only
        // at scale.
        let db = init_test_database();
        seed_session(&db, "t");
        for i in 0..500 {
            db.append_agent_chat_message("t", &format!(r#"{{"i":{i}}}"#))
                .unwrap();
        }
        let rows = db.list_agent_chat_messages("t");
        assert_eq!(rows.len(), 500);
        // Ordering is monotonic by insertion.
        for (i, row) in rows.iter().enumerate() {
            assert_eq!(row, &format!(r#"{{"i":{i}}}"#));
        }
    }

    #[test]
    fn agent_chat_messages_delete_for_thread_is_idempotent() {
        let db = init_test_database();
        seed_session(&db, "t");
        db.append_agent_chat_message("t", r#"{"i":1}"#).unwrap();
        db.delete_agent_chat_messages_for_thread("t").unwrap();
        assert!(db.list_agent_chat_messages("t").is_empty());
        // Second call on already-empty thread doesn't error.
        db.delete_agent_chat_messages_for_thread("t").unwrap();
        // Session row is untouched by message-only deletion.
        assert!(db.get_agent_chat_session("t").is_some());
    }

    #[test]
    fn agent_chat_messages_lifecycle_survives_reopen() {
        // End-to-end persistence guarantee: messages written to a
        // file-backed DB are still there after the process restarts.
        // Closes the loop — without this, replay-on-resume would only
        // work within a single session.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.db");

        {
            let conn = open_connection(&path).unwrap();
            create_schema(&conn).unwrap();
            let db = DatabaseStore {
                conn: Mutex::new(conn),
            };
            db.upsert_agent_chat_session("t", "ws", Some("/p"), "claude")
                .unwrap();
            db.append_agent_chat_message("t", r#"{"i":1}"#).unwrap();
            db.append_agent_chat_message("t", r#"{"i":2}"#).unwrap();
        }

        let conn = open_connection(&path).unwrap();
        create_schema(&conn).unwrap();
        let db = DatabaseStore {
            conn: Mutex::new(conn),
        };
        let rows = db.list_agent_chat_messages("t");
        assert_eq!(
            rows,
            vec![r#"{"i":1}"#.to_string(), r#"{"i":2}"#.to_string()]
        );
    }

    #[test]
    fn agent_chat_sessions_lifecycle_survives_reopen() {
        // Confirms the migration upgrades a legacy v1 DB cleanly:
        // schema_version starts at 2, and sessions round-trip.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chat.db");

        {
            let conn = open_connection(&path).unwrap();
            create_schema(&conn).unwrap();
            let db = DatabaseStore {
                conn: Mutex::new(conn),
            };
            db.upsert_agent_chat_session("t", "ws", Some("/p"), "claude")
                .unwrap();
            db.set_agent_chat_title("t", "Persisted").unwrap();
        }

        let conn = open_connection(&path).unwrap();
        create_schema(&conn).unwrap();
        let db = DatabaseStore {
            conn: Mutex::new(conn),
        };
        let version: u32 = db
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let rec = db.get_agent_chat_session("t").unwrap();
        assert_eq!(rec.title.as_deref(), Some("Persisted"));
    }

    // ── Hosts CRUD tests ──
    //
    // These exercise the soft-delete + dirty-flag invariants the sync
    // layer relies on. A bug here means hosts silently disappear or
    // duplicate on the user's other devices — much worse than a UI
    // glitch, so the coverage is intentionally thorough.

    #[test]
    fn hosts_insert_and_list() {
        let db = init_test_database();
        assert!(db.list_hosts().is_empty());

        let h = db.insert_host("homelab", "zeus@10.0.0.5").unwrap();
        assert_eq!(h.name, "homelab");
        assert_eq!(h.ssh_target, "zeus@10.0.0.5");
        assert!(
            h.dirty,
            "new rows must be marked dirty so sync picks them up"
        );
        assert!(h.server_id.is_none(), "fresh inserts have no server_id");
        assert!(h.deleted_at.is_none());

        let list = db.list_hosts();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, h.id);
    }

    #[test]
    fn hosts_list_ordered_case_insensitive() {
        let db = init_test_database();
        db.insert_host("zebra", "u@a").unwrap();
        db.insert_host("Apple", "u@b").unwrap();
        db.insert_host("banana", "u@c").unwrap();
        let names: Vec<String> = db.list_hosts().into_iter().map(|h| h.name).collect();
        assert_eq!(names, vec!["Apple", "banana", "zebra"]);
    }

    #[test]
    fn hosts_update_marks_dirty() {
        let db = init_test_database();
        let h = db.insert_host("orig", "old@host").unwrap();
        db.mark_host_synced(h.id, Some("srv-1")).unwrap();
        // After mark_synced, the row should be clean.
        let clean = db.list_hosts().into_iter().find(|x| x.id == h.id).unwrap();
        assert!(!clean.dirty);
        assert_eq!(clean.server_id.as_deref(), Some("srv-1"));

        let updated = db.update_host(h.id, "renamed", "new@host").unwrap();
        assert_eq!(updated.name, "renamed");
        assert_eq!(updated.ssh_target, "new@host");
        assert!(updated.dirty, "edits must re-mark the row dirty");
        assert_eq!(
            updated.server_id.as_deref(),
            Some("srv-1"),
            "server_id survives a rename so we update-not-recreate on push"
        );
    }

    #[test]
    fn hosts_update_unknown_id_errors() {
        let db = init_test_database();
        let result = db.update_host(9999, "x", "y");
        assert!(result.is_err());
    }

    #[test]
    fn hosts_delete_is_soft_and_dirty() {
        let db = init_test_database();
        let h = db.insert_host("doomed", "u@h").unwrap();
        db.delete_host(h.id).unwrap();

        // Soft-deleted rows do NOT appear in list_hosts.
        assert!(db.list_hosts().is_empty());

        // But they DO appear in list_hosts_for_sync so the tombstone
        // can be pushed to the server.
        let pending = db.list_hosts_for_sync();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].deleted_at.is_some());
        assert!(
            pending[0].dirty,
            "tombstones must be dirty so the sync layer pushes them"
        );
    }

    #[test]
    fn host_observation_columns_are_local_only() {
        let db = init_test_database();
        let h = db.insert_host("pandora", "u@pandora").unwrap();
        assert!(h.last_seen_at.is_none());
        assert!(h.disk_bytes.is_none());
        assert!(h.disk_measured_at.is_none());
        let find = || db.list_hosts().into_iter().find(|r| r.id == h.id).unwrap();

        // A tick that carried a disk measurement stamps all three.
        db.record_host_seen(h.id, "2026-08-27T10:00:00Z", Some(4096)).unwrap();
        let row = find();
        assert_eq!(row.last_seen_at.as_deref(), Some("2026-08-27T10:00:00Z"));
        assert_eq!(row.disk_bytes, Some(4096));
        assert_eq!(row.disk_measured_at.as_deref(), Some("2026-08-27T10:00:00Z"));

        // A tick without a walk advances last_seen_at only.
        db.record_host_seen(h.id, "2026-08-27T10:01:00Z", None).unwrap();
        let row = find();
        assert_eq!(row.last_seen_at.as_deref(), Some("2026-08-27T10:01:00Z"));
        assert_eq!(row.disk_bytes, Some(4096));
        assert_eq!(row.disk_measured_at.as_deref(), Some("2026-08-27T10:00:00Z"));

        // Observations never dirty the row: nothing here is worth a push.
        db.mark_host_synced(h.id, Some("srv-1")).unwrap();
        db.record_host_seen(h.id, "2026-08-27T10:02:00Z", Some(1)).unwrap();
        assert!(!find().dirty);

        // A server pull rewrites the synced fields and leaves the
        // local-only columns untouched.
        db.upsert_host_from_server(
            "srv-1",
            "renamed-elsewhere",
            "u@pandora",
            "2026-01-01T00:00:00Z",
            "2026-08-27T11:00:00Z",
            None,
        )
        .unwrap();
        let row = find();
        assert_eq!(row.name, "renamed-elsewhere");
        assert_eq!(row.last_seen_at.as_deref(), Some("2026-08-27T10:02:00Z"));
        assert_eq!(row.disk_bytes, Some(1));
        assert_eq!(row.disk_measured_at.as_deref(), Some("2026-08-27T10:02:00Z"));

        // The facts go with the row once the tombstone is purged; no
        // separate cleanup is needed.
        db.delete_host(h.id).unwrap();
        db.mark_host_synced(h.id, None).unwrap();
        db.purge_acknowledged_deletes().unwrap();
        assert!(db.list_hosts_for_sync().iter().all(|r| r.id != h.id));
    }

    #[test]
    fn hosts_delete_unknown_id_errors() {
        let db = init_test_database();
        assert!(db.delete_host(9999).is_err());
    }

    #[test]
    fn hosts_dirty_list_filters_correctly() {
        let db = init_test_database();
        let dirty = db.insert_host("a", "u@a").unwrap();
        let clean = db.insert_host("b", "u@b").unwrap();
        db.mark_host_synced(clean.id, Some("srv-b")).unwrap();

        let only_dirty = db.list_dirty_hosts();
        assert_eq!(only_dirty.len(), 1);
        assert_eq!(only_dirty[0].id, dirty.id);
    }

    #[test]
    fn hosts_purge_acknowledged_deletes() {
        let db = init_test_database();
        let h = db.insert_host("temp", "u@t").unwrap();
        db.delete_host(h.id).unwrap();
        // Before mark_synced: still a tombstone, must NOT be purged.
        db.purge_acknowledged_deletes().unwrap();
        assert_eq!(db.list_hosts_for_sync().len(), 1);
        // After mark_synced: tombstone is acknowledged, NOW purge.
        db.mark_host_synced(h.id, Some("srv-t")).unwrap();
        db.purge_acknowledged_deletes().unwrap();
        assert!(db.list_hosts_for_sync().is_empty());
    }

    #[test]
    fn hosts_upsert_from_server_new_then_update() {
        let db = init_test_database();
        // First sync: server has a row we don't.
        db.upsert_host_from_server(
            "srv-1",
            "from-cloud",
            "user@cloud",
            "2026-05-01 12:00:00",
            "2026-05-01 12:00:00",
            None,
        )
        .unwrap();
        let after_first = db.list_hosts();
        assert_eq!(after_first.len(), 1);
        assert_eq!(after_first[0].server_id.as_deref(), Some("srv-1"));
        assert!(
            !after_first[0].dirty,
            "server-sourced rows must NOT be dirty (they already match the server)"
        );

        // Second sync: server reports a rename. We must update in place,
        // not insert a duplicate.
        db.upsert_host_from_server(
            "srv-1",
            "renamed-from-cloud",
            "user@cloud",
            "2026-05-01 12:00:00",
            "2026-05-02 09:00:00",
            None,
        )
        .unwrap();
        let after_second = db.list_hosts();
        assert_eq!(after_second.len(), 1, "no duplicate row");
        assert_eq!(after_second[0].name, "renamed-from-cloud");

        // Third sync: server marks the row deleted.
        db.upsert_host_from_server(
            "srv-1",
            "renamed-from-cloud",
            "user@cloud",
            "2026-05-01 12:00:00",
            "2026-05-03 09:00:00",
            Some("2026-05-03 09:00:00"),
        )
        .unwrap();
        // list_hosts hides deleted rows; list_hosts_for_sync sees them.
        assert!(db.list_hosts().is_empty());
        let raw = db.list_hosts_for_sync();
        assert_eq!(raw.len(), 1);
        assert!(raw[0].deleted_at.is_some());
    }

    #[test]
    fn hosts_local_and_remote_coexist_until_paired() {
        // Realistic scenario: user adds a host on their laptop while
        // offline. Meanwhile their desktop synced a different host.
        // Once auth comes back and pull/push run, both rows should
        // coexist with distinct server_ids — no merge collision.
        let db = init_test_database();
        let local = db.insert_host("laptop-only", "u@laptop").unwrap();
        db.upsert_host_from_server(
            "srv-desktop",
            "desktop-only",
            "u@desktop",
            "2026-05-01 12:00:00",
            "2026-05-01 12:00:00",
            None,
        )
        .unwrap();
        let list = db.list_hosts();
        assert_eq!(list.len(), 2);
        // Pretend the local row got pushed; mark it synced.
        db.mark_host_synced(local.id, Some("srv-laptop")).unwrap();
        // Now both rows have distinct server_ids.
        let mut sids: Vec<String> = db
            .list_hosts()
            .into_iter()
            .filter_map(|h| h.server_id)
            .collect();
        sids.sort();
        assert_eq!(
            sids,
            vec!["srv-desktop".to_string(), "srv-laptop".to_string()]
        );
    }

    // ── Automations ──

    fn sample_automation(name: &str) -> AutomationInput {
        AutomationInput {
            name: name.to_string(),
            prompt: "Triage open issues".to_string(),
            agent: "claude".to_string(),
            schedule: "DTSTART:20260101T090000Z\nRRULE:FREQ=DAILY".to_string(),
            timezone: "UTC".to_string(),
            host_id: None,
            project_path: Some("/home/user/repo".to_string()),
            project_remote: None,
            retention_limit: 10,
        }
    }

    #[test]
    fn automation_crud() {
        let db = init_test_database();
        assert_eq!(db.list_automations().len(), 0);

        let created = db
            .insert_automation(&sample_automation("Daily triage"))
            .unwrap();
        assert_eq!(created.name, "Daily triage");
        assert!(created.enabled, "new automations default to enabled");
        assert_eq!(created.retention_limit, 10);
        assert!(created.dirty, "a fresh insert is dirty until synced");
        assert!(
            created.next_run_at.is_none(),
            "next_run_at is derived state set by the command layer, not the insert"
        );

        let fetched = db.get_automation(created.id).unwrap();
        assert_eq!(fetched.prompt, "Triage open issues");
        assert_eq!(db.list_automations().len(), 1);

        let mut edit = sample_automation("Daily triage");
        edit.prompt = "Triage and label issues".to_string();
        let updated = db.update_automation(created.id, &edit).unwrap();
        assert_eq!(updated.prompt, "Triage and label issues");

        // Soft-delete drops it from both `list` and `get`.
        db.delete_automation(created.id).unwrap();
        assert_eq!(db.list_automations().len(), 0);
        assert!(db.get_automation(created.id).is_none());

        // A soft-deleted automation can no longer be updated.
        assert!(db.update_automation(created.id, &edit).is_err());
    }

    #[test]
    fn automation_enabled_toggle() {
        let db = init_test_database();
        let a = db
            .insert_automation(&sample_automation("Nightly build"))
            .unwrap();
        assert!(a.enabled);

        let paused = db.set_automation_enabled(a.id, false).unwrap();
        assert!(!paused.enabled);

        let resumed = db.set_automation_enabled(a.id, true).unwrap();
        assert!(resumed.enabled);
    }

    #[test]
    fn automation_next_run_is_settable_and_clearable() {
        let db = init_test_database();
        let a = db
            .insert_automation(&sample_automation("Weekly report"))
            .unwrap();

        db.set_automation_next_run(a.id, Some("2026-02-01T09:00:00+00:00"))
            .unwrap();
        assert_eq!(
            db.get_automation(a.id).unwrap().next_run_at.as_deref(),
            Some("2026-02-01T09:00:00+00:00")
        );

        db.set_automation_next_run(a.id, None).unwrap();
        assert!(db.get_automation(a.id).unwrap().next_run_at.is_none());
    }

    #[test]
    fn automation_run_insert_is_idempotent_per_minute() {
        let db = init_test_database();
        let a = db
            .insert_automation(&sample_automation("Hourly sync"))
            .unwrap();

        let first = db
            .record_automation_run(a.id, "running", "2026-01-01T09:00:00Z", None, None)
            .unwrap();
        assert!(first.is_some());

        // A re-delivered fire for the same minute is a no-op.
        let dup = db
            .record_automation_run(a.id, "running", "2026-01-01T09:00:00Z", None, None)
            .unwrap();
        assert!(dup.is_none());

        // A different minute is a distinct run.
        let next = db
            .record_automation_run(a.id, "running", "2026-01-01T10:00:00Z", None, None)
            .unwrap();
        assert!(next.is_some());

        assert_eq!(db.list_automation_runs(a.id, 10).len(), 2);
    }

    #[test]
    fn automation_run_lifecycle_reaches_a_terminal_state() {
        let db = init_test_database();
        let a = db
            .insert_automation(&sample_automation("Deploy check"))
            .unwrap();

        let run = db
            .record_automation_run(a.id, "scheduled", "2026-03-01T08:00:00Z", Some(1), None)
            .unwrap()
            .unwrap();
        assert_eq!(run.status, "scheduled");
        assert!(run.started_at.is_none());

        db.mark_automation_run_started(run.id).unwrap();
        db.finish_automation_run(
            run.id,
            "succeeded",
            Some("ws-42"),
            Some("automation-deploy-check-20260301-080000"),
            None,
            None,
        )
        .unwrap();

        let runs = db.list_automation_runs(a.id, 10);
        assert_eq!(runs.len(), 1);
        let finished = &runs[0];
        assert_eq!(finished.status, "succeeded");
        assert!(finished.started_at.is_some());
        assert!(finished.finished_at.is_some());
        assert_eq!(finished.workspace_id.as_deref(), Some("ws-42"));
        assert_eq!(
            finished.branch.as_deref(),
            Some("automation-deploy-check-20260301-080000")
        );
    }

    #[test]
    fn automation_runs_are_listed_newest_fire_first() {
        let db = init_test_database();
        let a = db
            .insert_automation(&sample_automation("Ordering"))
            .unwrap();
        db.record_automation_run(a.id, "succeeded", "2026-01-01T09:00:00Z", None, None)
            .unwrap();
        db.record_automation_run(a.id, "succeeded", "2026-01-03T09:00:00Z", None, None)
            .unwrap();
        db.record_automation_run(a.id, "succeeded", "2026-01-02T09:00:00Z", None, None)
            .unwrap();

        let runs = db.list_automation_runs(a.id, 10);
        let order: Vec<&str> = runs.iter().map(|r| r.scheduled_for.as_str()).collect();
        assert_eq!(
            order,
            vec![
                "2026-01-03T09:00:00Z",
                "2026-01-02T09:00:00Z",
                "2026-01-01T09:00:00Z",
            ]
        );
    }

    #[test]
    fn reconcile_stale_runs_fails_only_non_terminal_runs() {
        let db = init_test_database();
        let a = db
            .insert_automation(&sample_automation("Reconcile"))
            .unwrap();
        let scheduled = db
            .record_automation_run(a.id, "scheduled", "2026-01-01T09:00:00Z", None, None)
            .unwrap()
            .unwrap();
        let running = db
            .record_automation_run(a.id, "running", "2026-01-01T10:00:00Z", None, None)
            .unwrap()
            .unwrap();
        let done = db
            .record_automation_run(a.id, "succeeded", "2026-01-01T11:00:00Z", None, None)
            .unwrap()
            .unwrap();

        // A ceiling far in the future makes every run older than it.
        let reconciled = db.reconcile_stale_runs("2999-01-01T00:00:00Z");
        assert_eq!(reconciled, 2, "only the two non-terminal runs are stale");

        let runs = db.list_automation_runs(a.id, 10);
        let status = |id: i64| runs.iter().find(|r| r.id == id).unwrap().status.clone();
        assert_eq!(status(scheduled.id), "failed");
        assert_eq!(status(running.id), "failed");
        assert_eq!(status(done.id), "succeeded", "terminal runs are untouched");
    }

    #[test]
    fn reconcile_stale_runs_leaves_recent_runs_alone() {
        let db = init_test_database();
        let a = db.insert_automation(&sample_automation("Recent")).unwrap();
        db.record_automation_run(a.id, "running", "2026-01-01T09:00:00Z", None, None)
            .unwrap();
        // A ceiling in the distant past — the run's created_at (now) is
        // newer, so nothing is stale.
        assert_eq!(db.reconcile_stale_runs("2000-01-01T00:00:00Z"), 0);
        assert_eq!(db.list_automation_runs(a.id, 10)[0].status, "running");
    }

    #[test]
    fn reconcile_stale_runs_compares_mixed_timestamp_formats() {
        // `created_at` is stored in SQLite's `'YYYY-MM-DD HH:MM:SS'`
        // form, while the real callers pass an RFC 3339 ceiling
        // (`now - 6h`). A raw string `<` would compare the space
        // against the `T` at offset 10 and wrongly judge a run from
        // earlier *today* as stale. The previous tests used year-2000
        // and year-2999 ceilings, where the year differs first and
        // hides the bug — this one uses a same-day ceiling so only
        // correct `datetime()` normalisation passes it.
        let db = init_test_database();
        let a = db
            .insert_automation(&sample_automation("MixedFmt"))
            .unwrap();
        db.record_automation_run(a.id, "running", "2026-01-01T09:00:00Z", None, None)
            .unwrap();
        let ceiling = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
        assert_eq!(
            db.reconcile_stale_runs(&ceiling),
            0,
            "a run created moments ago is newer than a ceiling one hour back"
        );
        assert_eq!(db.list_automation_runs(a.id, 10)[0].status, "running");
    }
}
