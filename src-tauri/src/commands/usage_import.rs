//! Materialize usage from each provider's durable local history.
//!
//! Claude Code and Codex write JSONL transcripts; OpenCode stores the
//! equivalent assistant-message records in SQLite (with legacy JSON files
//! on older installs). Those provider-owned records are the single source of
//! truth for every session on this machine, regardless of whether Codemux,
//! another app, or a terminal launched it.
//!
//! Every record has a provider-native `import_key`. Re-scanning is
//! idempotent, but an active response may grow, so the cache uses a null-safe
//! upsert instead of freezing the first partial snapshot it sees.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::agent_provider::pricing;
use crate::database::{DatabaseStore, ProviderUsageCacheRow};

/// Bump this whenever a change would make previously-imported rows
/// wrong — a parse fix, a new field, a price correction.
///
/// The ledger is a cache, so a mismatch clears it and rebuilds from the
/// histories that remain on disk. Version 3 also removes old live-ledger
/// rows; otherwise Codemux-launched work would appear once from runtime
/// events and once from provider history.
///
/// | v | why |
/// |---|---|
/// | 1 | initial import (implicit — installs predating this constant) |
/// | 2 | per-turn Codex, Claude cache TTLs, corrected model prices |
/// | 3 | provider history authoritative; growing rows upsert; OpenCode imported |
pub const IMPORT_VERSION: i64 = 3;

/// Settings key holding the [`IMPORT_VERSION`] the current rows were
/// produced by.
const IMPORT_VERSION_KEY: &str = "usage.import_version";

/// What one scan did, for the footer note.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageImportReport {
    pub files_scanned: i64,
    pub sessions_found: i64,
    /// New or changed materialized records.
    pub rows_updated: i64,
    /// True when this scan found a stale [`IMPORT_VERSION`] and rebuilt
    /// the materialized cache from scratch.
    pub reimported: bool,
}

/// A normalized, non-overlapping token split ready for the ledger.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportedUsage {
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    /// Subset of `cache_write`: the part written at the **1-hour** TTL,
    /// which Anthropic bills at 2x input rather than 1.25x.
    ///
    /// Not a fifth ledger column — it exists only long enough to price
    /// the row. The ledger stores `cache_write` as the total, preserving
    /// the four-way non-overlapping invariant everything downstream
    /// rests on.
    pub cache_write_1h: i64,
    /// Subset of `output`.
    pub reasoning: i64,
}

impl ImportedUsage {
    fn is_empty(&self) -> bool {
        self.input == 0 && self.output == 0 && self.cache_read == 0 && self.cache_write == 0
    }
}

/// One row an importer wants written.
#[derive(Debug, Clone)]
pub struct ImportedRow {
    pub import_key: String,
    pub created_at_ms: i64,
    pub session_id: String,
    pub provider: &'static str,
    pub model: Option<String>,
    pub subagent: bool,
    pub usage: ImportedUsage,
    /// Upstream-computed cost when the history includes one (OpenCode).
    pub reported_cost_usd: Option<f64>,
}

// ── Claude: ~/.claude/projects/**/*.jsonl ──

/// Parse one Claude transcript into ledger rows.
///
/// Shape verified against real logs on this machine: each line is a
/// record with a `type`; `"assistant"` records carry `message.usage`
/// (`input_tokens`, `cache_creation_input_tokens`,
/// `cache_read_input_tokens`, `output_tokens` — already disjoint, as in
/// the API), `message.model`, `message.id`, plus a top-level
/// `sessionId`, `timestamp` and `isSidechain`.
///
/// **The dedup invariant, measured rather than assumed:** across 80
/// sampled transcripts, *every one* contained repeated `message.id`s
/// whose usage blocks were identical or growing — the CLI writes one
/// record per content block of a single API response, each carrying the
/// same cumulative usage. Summing them overcounts by the number of
/// blocks (commonly 4x). So rows are grouped by `message.id` and the
/// per-field **maximum** is taken, which is that response's final
/// cumulative figure. This mirrors the live adapter's high-water mark.
///
/// `ccusage`, the community-standard accountant for these same logs,
/// dedups on a `message.id` + `requestId` pair and keeps the *first*
/// occurrence. Re-measured across 200 transcripts / 19,120 assistant
/// records on this machine: **no `message.id` ever carried more than one
/// `requestId`**, and every repeat's usage block was identical, so the
/// two rules produce the same number. The per-field max is kept because
/// it is the safer of the two if a repeat ever does grow — but the
/// `import_key` now carries the `requestId` too, so the unique index
/// agrees with ccusage's key rather than merely with itself.
///
/// The key is **global**, not session-scoped: Claude Code copies a
/// message's records forward when a session is resumed or forked, so the
/// same response can legitimately appear in two transcripts. (Unobserved
/// here — 0 of 9,537 distinct ids appeared in more than one file — but
/// the index makes the protection free.)
pub fn parse_claude_transcript(lines: impl Iterator<Item = String>) -> Vec<ImportedRow> {
    // message.id -> (usage high-water, first timestamp, model, subagent,
    //                requestId)
    let mut by_message: HashMap<String, ClaudeMessageAcc> = HashMap::new();
    let mut session_id: Option<String> = None;

    for line in lines {
        let Ok(record) = serde_json::from_str::<serde_json::Value>(&line) else {
            // A truncated or malformed line must never abort the file —
            // logs are appended live and the tail can be half-written.
            continue;
        };
        if record.get("type").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }
        let sid = record.get("sessionId").and_then(|v| v.as_str());
        if let Some(sid) = sid {
            session_id.get_or_insert_with(|| sid.to_string());
        }
        let Some(message) = record.get("message") else {
            continue;
        };
        let Some(usage) = message.get("usage") else {
            continue;
        };
        let Some(message_id) = message.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        let field = |key: &str| usage.get(key).and_then(|v| v.as_i64()).unwrap_or(0);
        // `cache_creation` breaks the cache-write total down by TTL, and
        // the two tiers bill differently: 2x input for 1-hour entries
        // against 1.25x for 5-minute ones. The mix is real and swings by
        // model and by day — measured across all 1,334 transcripts on
        // this machine, 70.9M of 176.6M cache-creation tokens (40.2%)
        // are 1-hour, ranging from 18.7% on one day to 100% on another,
        // and from 24.6% for opus-5 to 63.5% for fable-5. Pricing the
        // lot at 1.25x understates real Claude cache cost by ~$419 at
        // list over that corpus.
        //
        // An older transcript without the sub-object yields 0, i.e. the
        // previous all-5-minute assumption.
        let ephemeral = usage.get("cache_creation");
        let ephemeral_field = |key: &str| {
            ephemeral
                .and_then(|c| c.get(key))
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
        };
        let cache_write_1h = ephemeral_field("ephemeral_1h_input_tokens");
        // The top-level counter can UNDER-report when one response ran
        // several server-side iterations: 5 messages of 32,621 here have
        // `cache_creation_input_tokens` smaller than the ephemeral tiers
        // sum to (`usage.iterations` has two entries in every such case).
        // Taking the larger closes exactly the 0.13% cache-create gap
        // against ccusage — the per-day deltas matched these messages to
        // the token. Note ccusage does *not* sum the `iterations` array;
        // that would over-report by an order of magnitude.
        let cache_write = field("cache_creation_input_tokens")
            .max(cache_write_1h + ephemeral_field("ephemeral_5m_input_tokens"));
        let observed = ImportedUsage {
            input: field("input_tokens"),
            output: field("output_tokens"),
            cache_read: field("cache_read_input_tokens"),
            cache_write,
            cache_write_1h,
            // Anthropic reports no reasoning split; thinking is billed
            // and reported as plain output.
            reasoning: 0,
        };
        let timestamp = record
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(parse_iso8601_ms)
            .unwrap_or(0);
        let model = message
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        // `<synthetic>` records are CLI-generated placeholders (interrupt
        // notices and the like), not API responses. They were never
        // billed, so they must not appear in an accounting ledger even
        // with a zero price.
        if model.as_deref() == Some("<synthetic>") {
            continue;
        }
        // `isSidechain` marks a subagent's transcript branch.
        let subagent = record
            .get("isSidechain")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let request_id = record
            .get("requestId")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let entry = by_message
            .entry(message_id.to_string())
            .or_insert_with(|| ClaudeMessageAcc {
                usage: ImportedUsage::default(),
                created_at_ms: timestamp,
                model: model.clone(),
                subagent,
                request_id: request_id.clone(),
            });
        // Per-field max = the response's final cumulative figure.
        entry.usage.input = entry.usage.input.max(observed.input);
        entry.usage.output = entry.usage.output.max(observed.output);
        entry.usage.cache_read = entry.usage.cache_read.max(observed.cache_read);
        entry.usage.cache_write = entry.usage.cache_write.max(observed.cache_write);
        entry.usage.cache_write_1h = entry.usage.cache_write_1h.max(observed.cache_write_1h);
        if entry.model.is_none() {
            entry.model = model;
        }
        if entry.request_id.is_none() {
            entry.request_id = request_id;
        }
    }

    let Some(session_id) = session_id else {
        return Vec::new();
    };

    let mut rows: Vec<ImportedRow> = by_message
        .into_iter()
        .filter(|(_, acc)| !acc.usage.is_empty())
        .map(|(message_id, acc)| ImportedRow {
            // ccusage's key, and deliberately NOT session-scoped — see
            // the function doc. A missing `requestId` (10 of 19,120
            // records here) degrades to a bare message id, which is
            // still globally unique in the measured data.
            import_key: format!(
                "claude:{message_id}:{}",
                acc.request_id.as_deref().unwrap_or("-")
            ),
            created_at_ms: acc.created_at_ms,
            session_id: session_id.clone(),
            provider: "claude",
            model: acc.model,
            subagent: acc.subagent,
            usage: acc.usage,
            reported_cost_usd: None,
        })
        .collect();
    rows.sort_by_key(|r| r.created_at_ms);
    rows
}

/// Per-`message.id` accumulator for [`parse_claude_transcript`].
struct ClaudeMessageAcc {
    usage: ImportedUsage,
    created_at_ms: i64,
    model: Option<String>,
    subagent: bool,
    request_id: Option<String>,
}

// ── Codex: ~/.codex/sessions/**/*.jsonl ──

/// Parse one Codex rollout into **one row per turn**.
///
/// Shape verified against real rollouts on this machine: a
/// `session_meta` record carries `payload.id` (the rollout id),
/// `turn_context` records carry `payload.model`, and `event_msg` records
/// with `payload.type == "token_count"` carry an `info` object holding
/// **both** counters:
///
/// * `total_token_usage` — the cumulative session total, re-sent every
///   update.
/// * `last_token_usage` — that turn's delta.
///
/// Both carry `input_tokens`, `cached_input_tokens`,
/// `cache_write_input_tokens`, `output_tokens`, `reasoning_output_tokens`
/// and `total_tokens`. `cached_input_tokens` is a **subset** of
/// `input_tokens` (`total_tokens == input_tokens + output_tokens` holds
/// in the samples), exactly as in the app-server protocol, so fresh
/// input is `input - cached - cache_write`.
///
/// **This used to write one aggregate row per rollout, and that was
/// wrong in two ways** — both fixed by importing the per-turn deltas:
///
/// 1. **Day bucketing.** The aggregate was stamped with the rollout's
///    *start* time, so a session spanning three days put all three days'
///    tokens on day one. Long rollouts are the norm: one file here holds
///    295 turns over several hours, another 488.
/// 2. **Model attribution.** The old parse took the *first*
///    `turn_context` model and applied it to the whole session, so a
///    mid-session model switch was mispriced end to end. The model is
///    now carried forward and re-read on every `turn_context`.
///
/// The deltas reconcile: summing `last_token_usage` across a rollout
/// reproduces the final `total_token_usage` exactly in 4 of 6 sampled
/// files, and to within 0.15% in the rest (attributable to a repeated
/// event — see the duplicate guard below).
///
/// Rollouts predating `last_token_usage` (2 of 148 files here) fall back
/// to the old single-aggregate behavior rather than importing nothing.
pub fn parse_codex_rollout(lines: impl Iterator<Item = String>) -> Vec<ImportedRow> {
    let mut session_id: Option<String> = None;
    let mut model: Option<String> = None;
    let mut rows: Vec<ImportedRow> = Vec::new();
    let mut turn_index: usize = 0;
    // Consecutive-duplicate guard: an identical `last_token_usage` in
    // two adjacent events is a re-send, not a second turn whose token
    // counts coincided to the token.
    let mut previous_delta: Option<ImportedUsage> = None;

    // Fallback state, used only when the file has no `last_token_usage`.
    let mut best = ImportedUsage::default();
    let mut best_total: i64 = -1;
    let mut created_at_ms: i64 = 0;
    let mut last_ts: i64 = 0;
    let mut saw_delta = false;
    let mut subagent = false;

    for line in lines {
        let Ok(record) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let ts = record
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(parse_iso8601_ms)
            .unwrap_or(0);
        match record.get("type").and_then(|v| v.as_str()) {
            Some("session_meta") => {
                if let Some(id) = record
                    .get("payload")
                    .and_then(|p| p.get("id"))
                    .and_then(|v| v.as_str())
                {
                    session_id.get_or_insert_with(|| id.to_string());
                }
                let payload = record.get("payload");
                subagent = payload
                    .and_then(|p| p.get("source"))
                    .and_then(|source| source.get("subagent"))
                    .is_some()
                    || payload
                        .and_then(|p| p.get("parent_thread_id"))
                        .and_then(|v| v.as_str())
                        .is_some();
                if created_at_ms == 0 {
                    created_at_ms = ts;
                }
            }
            Some("turn_context") => {
                // Re-read every time, not just the first: a rollout can
                // switch models mid-session and each turn must be priced
                // at the model that actually served it.
                if let Some(next) = record
                    .get("payload")
                    .and_then(|p| p.get("model"))
                    .and_then(|v| v.as_str())
                {
                    model = Some(next.to_string());
                }
            }
            Some("event_msg") => {
                let payload = record.get("payload");
                if payload.and_then(|p| p.get("type")).and_then(|v| v.as_str())
                    != Some("token_count")
                {
                    continue;
                }
                let Some(info) = payload.and_then(|p| p.get("info")) else {
                    continue;
                };
                if ts > last_ts {
                    last_ts = ts;
                }

                if let Some(delta) = info.get("last_token_usage").map(split_from_codex_usage) {
                    saw_delta = true;
                    if delta.is_empty() || previous_delta == Some(delta) {
                        continue;
                    }
                    previous_delta = Some(delta);
                    let Some(session) = session_id.as_deref() else {
                        // A `token_count` before the session meta has no
                        // id to key on. Dropping it (rather than keying
                        // on a placeholder) keeps the import idempotent.
                        continue;
                    };
                    rows.push(ImportedRow {
                        import_key: format!("codex:{session}:{turn_index}"),
                        // The turn's own timestamp, which is the whole
                        // point of the per-turn grain.
                        created_at_ms: if ts > 0 { ts } else { created_at_ms },
                        session_id: session.to_string(),
                        provider: "codex",
                        model: model.clone(),
                        subagent,
                        usage: delta,
                        reported_cost_usd: None,
                    });
                    turn_index += 1;
                    continue;
                }

                // ── legacy fallback: cumulative totals only ──
                let Some(total) = info.get("total_token_usage") else {
                    continue;
                };
                let observed = split_from_codex_usage(total);
                let total_tokens = total
                    .get("total_tokens")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                // Keep the largest cumulative reading rather than simply
                // the last: a truncated tail line must not shrink the
                // session's total.
                if total_tokens >= best_total {
                    best_total = total_tokens;
                    best = observed;
                }
            }
            _ => {}
        }
    }

    let Some(session_id) = session_id else {
        return Vec::new();
    };
    if saw_delta {
        return rows;
    }
    if best.is_empty() {
        return Vec::new();
    }
    let row = ImportedRow {
        // One row per rollout, so the rollout id alone is a stable key.
        import_key: format!("codex:{session_id}"),
        created_at_ms: if created_at_ms > 0 {
            created_at_ms
        } else {
            last_ts
        },
        session_id,
        provider: "codex",
        model,
        subagent,
        usage: best,
        reported_cost_usd: None,
    };
    vec![row]
}

/// Normalize either Codex usage object into the disjoint ledger split.
///
/// `cached_input_tokens` and `cache_write_input_tokens` are both carved
/// **out of** `input_tokens`, so fresh input is what remains. The
/// cache-write counter *is* present in the local rollout format — an
/// earlier version of this file claimed otherwise and hardcoded 0. It
/// reads 0 on every rollout on this machine (13,004 occurrences, all
/// zero), which is presumably how the wrong conclusion was reached, but
/// reading it costs nothing and a nonzero value would otherwise be
/// silently folded into fresh input at the wrong rate.
fn split_from_codex_usage(usage: &serde_json::Value) -> ImportedUsage {
    let field = |key: &str| usage.get(key).and_then(|v| v.as_i64()).unwrap_or(0);
    let cached = field("cached_input_tokens");
    let cache_write = field("cache_write_input_tokens");
    ImportedUsage {
        input: (field("input_tokens") - cached - cache_write).max(0),
        output: field("output_tokens"),
        cache_read: cached,
        cache_write,
        // OpenAI publishes no longer-TTL cache tier.
        cache_write_1h: 0,
        reasoning: field("reasoning_output_tokens"),
    }
}

// ── OpenCode: ~/.local/share/opencode/opencode*.db + legacy JSON ──

/// Normalize one OpenCode assistant-message record.
///
/// OpenCode's buckets are disjoint. `reasoning` sits beside `output`, but is
/// billed as output, so it is folded into the output total and also retained
/// as its informational subset. Message ids are stable while a streaming
/// response grows, which is why the database layer upserts this key.
fn parse_opencode_message(
    data: &serde_json::Value,
    fallback_message_id: &str,
    session_id: &str,
    fallback_created_at_ms: i64,
    subagent: bool,
) -> Option<ImportedRow> {
    if data.get("role").and_then(|v| v.as_str()) != Some("assistant") {
        return None;
    }
    let tokens = data.get("tokens")?;
    let field = |value: &serde_json::Value, key: &str| {
        value.get(key).and_then(|v| v.as_i64()).unwrap_or(0).max(0)
    };
    let reasoning = field(tokens, "reasoning");
    let cache = tokens.get("cache").unwrap_or(&serde_json::Value::Null);
    let usage = ImportedUsage {
        input: field(tokens, "input"),
        output: field(tokens, "output").saturating_add(reasoning),
        cache_read: field(cache, "read"),
        cache_write: field(cache, "write"),
        cache_write_1h: 0,
        reasoning,
    };
    if usage.is_empty() {
        return None;
    }

    let message_id = data
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or(fallback_message_id);
    let created_at_ms = data
        .get("time")
        .and_then(|v| v.get("created"))
        .and_then(|v| v.as_i64())
        .unwrap_or(fallback_created_at_ms);
    let provider_id = data.get("providerID").and_then(|v| v.as_str());
    let model_id = data.get("modelID").and_then(|v| v.as_str());
    let model = match (provider_id, model_id) {
        (Some(provider), Some(model)) => Some(format!("{provider}/{model}")),
        (None, Some(model)) => Some(model.to_string()),
        _ => None,
    };

    Some(ImportedRow {
        import_key: format!("opencode:{message_id}"),
        created_at_ms,
        session_id: session_id.to_string(),
        provider: "opencode",
        model,
        subagent,
        usage,
        reported_cost_usd: data.get("cost").and_then(|v| v.as_f64()),
    })
}

fn read_opencode_db(path: &Path) -> Result<Vec<ImportedRow>, String> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("Could not open {}: {e}", path.display()))?;

    // Parent sessions are OpenCode's subagents. Older schemas may not have
    // `parent_id`; in that case the usage is still valid, just unclassified.
    let mut child_sessions = HashSet::new();
    if let Ok(mut stmt) =
        conn.prepare("SELECT id FROM session WHERE parent_id IS NOT NULL AND parent_id != ''")
    {
        if let Ok(ids) = stmt.query_map([], |row| row.get::<_, String>(0)) {
            child_sessions.extend(ids.filter_map(Result::ok));
        }
    }

    let mut stmt = conn
        .prepare("SELECT id, session_id, time_created, data FROM message")
        .map_err(|e| {
            format!(
                "Could not read OpenCode messages in {}: {e}",
                path.display()
            )
        })?;
    let messages = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| {
            format!(
                "Could not query OpenCode messages in {}: {e}",
                path.display()
            )
        })?;

    let mut rows = Vec::new();
    for message in messages {
        let Ok((message_id, session_id, created_at_ms, raw)) = message else {
            continue;
        };
        let Ok(data) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        if let Some(row) = parse_opencode_message(
            &data,
            &message_id,
            &session_id,
            created_at_ms,
            child_sessions.contains(&session_id),
        ) {
            rows.push(row);
        }
    }
    Ok(rows)
}

fn find_files_with_extension(root: &Path, extension: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            find_files_with_extension(&path, extension, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some(extension) {
            out.push(path);
        }
    }
}

fn opencode_storage_root() -> Option<PathBuf> {
    dirs::data_local_dir().map(|root| root.join("opencode"))
}

fn opencode_databases(root: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = HashSet::new();
    if let Some(explicit) = std::env::var_os("OPENCODE_DB") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            paths.insert(path);
        }
    }
    if let Some(root) = root {
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = path.file_name().and_then(|v| v.to_str()).unwrap_or("");
                if path.is_file() && name.starts_with("opencode") && name.ends_with(".db") {
                    paths.insert(path);
                }
            }
        }
    }
    let mut paths: Vec<_> = paths.into_iter().collect();
    paths.sort();
    paths
}

/// A SQLite database's observable content signature. Include the WAL because
/// active OpenCode sessions may not have checkpointed their newest messages
/// into the main database file yet.
fn sqlite_signature(path: &Path) -> Option<(i64, i64)> {
    let mut mtime_ms = 0;
    let mut size = 0_i64;
    let mut files = vec![path.to_path_buf()];
    let wal = PathBuf::from(format!("{}-wal", path.to_string_lossy()));
    files.push(wal);
    let mut found = false;
    for file in files {
        let Ok(meta) = std::fs::metadata(file) else {
            continue;
        };
        found = true;
        size = size.saturating_add(meta.len() as i64);
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        mtime_ms = mtime_ms.max(modified);
    }
    found.then_some((mtime_ms, size))
}

/// Milliseconds since the epoch for an ISO-8601 UTC timestamp.
///
/// Hand-rolled for the same reason `civil_from_days` is: pulling in a
/// date crate to read `2026-07-10T12:33:12.425Z` would be the larger
/// dependency. Anything unparseable yields `None` and the record falls
/// back to a zero timestamp rather than being dropped.
pub fn parse_iso8601_ms(raw: &str) -> Option<i64> {
    let bytes = raw.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let num = |a: usize, b: usize| raw.get(a..b)?.parse::<i64>().ok();
    let year = num(0, 4)?;
    let month = num(5, 7)?;
    let day = num(8, 10)?;
    let hour = num(11, 13)?;
    let minute = num(14, 16)?;
    let second = num(17, 19)?;
    let millis = if bytes.len() >= 23 && bytes[19] == b'.' {
        num(20, 23).unwrap_or(0)
    } else {
        0
    };
    let days = days_from_civil(year, month, day);
    Some(((days * 24 + hour) * 60 + minute) * 60_000 + second * 1000 + millis)
}

/// Howard Hinnant's `days_from_civil` — the inverse of the
/// `civil_from_days` already used for bucket labels.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Every `*.jsonl` under `root`, recursively. Missing root → empty.
fn find_jsonl(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            find_jsonl(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn file_signature(path: &Path) -> Option<(i64, i64)> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Some((mtime_ms, meta.len() as i64))
}

fn tree_signature(files: &[PathBuf]) -> Option<(i64, i64)> {
    let mut mtime_ms = 0;
    let mut size = files.len() as i64;
    for path in files {
        let Some((modified, bytes)) = file_signature(path) else {
            continue;
        };
        mtime_ms = mtime_ms.max(modified);
        size = size.saturating_add(bytes);
    }
    (!files.is_empty()).then_some((mtime_ms, size))
}

fn source_changed(db: &DatabaseStore, key: &str, signature: (i64, i64)) -> bool {
    !matches!(
        db.usage_import_state(key),
        Ok(Some((mtime, size, _))) if (mtime, size) == signature
    )
}

fn materialize_rows(
    db: &DatabaseStore,
    rows: Vec<ImportedRow>,
    report: &mut UsageImportReport,
    sessions: &mut HashSet<String>,
) -> Result<(), String> {
    let mut cache_rows = Vec::with_capacity(rows.len());
    for row in rows {
        sessions.insert(format!("{}:{}", row.provider, row.session_id));
        let (cost, cost_source) = match row.reported_cost_usd {
            Some(cost) if cost.is_finite() && cost >= 0.0 => (Some(cost), Some("provider")),
            _ => {
                let cost = pricing::cost_for_with_1h(
                    row.model.as_deref(),
                    row.usage.input.max(0) as u64,
                    row.usage.output.max(0) as u64,
                    row.usage.cache_read.max(0) as u64,
                    row.usage.cache_write.max(0) as u64,
                    row.usage.cache_write_1h.max(0) as u64,
                );
                let source = cost.map(|_| "table");
                (cost, source)
            }
        };
        cache_rows.push(ProviderUsageCacheRow {
            import_key: row.import_key,
            created_at: row.created_at_ms,
            thread_id: format!("{}:{}", row.provider, row.session_id),
            provider: row.provider.to_string(),
            model: row.model,
            subagent: row.subagent,
            input_tokens: row.usage.input,
            output_tokens: row.usage.output,
            cache_read_tokens: row.usage.cache_read,
            cache_write_tokens: row.usage.cache_write,
            reasoning_tokens: row.usage.reasoning,
            cost_usd: cost,
            cost_source: cost_source.map(str::to_string),
        });
    }
    report.rows_updated += db.upsert_provider_usage_rows(&cache_rows)? as i64;
    Ok(())
}

/// Scan Claude, Codex, and OpenCode provider histories.
///
/// JSONL files are re-read in full when their size or mtime changes because
/// the metadata needed to interpret a tail record may occur at the start.
/// SQLite scans include the WAL in their signature. Provider-native keys make
/// overlapping databases, migrated legacy JSON, and repeated scans harmless.
#[tauri::command]
pub async fn usage_scan_provider_history(
    db: State<'_, DatabaseStore>,
) -> Result<UsageImportReport, String> {
    let home = dirs::home_dir().ok_or("Could not determine home directory")?;
    let mut report = UsageImportReport::default();
    let mut sessions = HashSet::new();
    let mut completed_states = Vec::new();
    let mut pending_rows = Vec::new();

    let stale_version = purge_if_importer_changed(&db)?;
    report.reimported = stale_version;

    let targets: [(PathBuf, &'static str); 2] = [
        (home.join(".claude").join("projects"), "claude"),
        (home.join(".codex").join("sessions"), "codex"),
    ];
    for (root, provider) in targets {
        let mut files = Vec::new();
        find_jsonl(&root, &mut files);
        for path in files {
            let Some(signature) = file_signature(&path) else {
                continue;
            };
            let key = path.to_string_lossy().to_string();
            if !source_changed(&db, &key, signature) {
                continue;
            }
            report.files_scanned += 1;
            let Ok(file) = std::fs::File::open(&path) else {
                continue;
            };
            let lines = BufReader::new(file).lines().map_while(Result::ok);
            let rows = match provider {
                "claude" => parse_claude_transcript(lines),
                _ => parse_codex_rollout(lines),
            };
            pending_rows.extend(rows);
            completed_states.push((key, signature.0, signature.1));
        }
    }

    let opencode_root = opencode_storage_root();
    for path in opencode_databases(opencode_root.as_deref()) {
        let Some(signature) = sqlite_signature(&path) else {
            continue;
        };
        let key = path.to_string_lossy().to_string();
        if !source_changed(&db, &key, signature) {
            continue;
        }
        report.files_scanned += 1;
        match read_opencode_db(&path) {
            Ok(rows) => {
                pending_rows.extend(rows);
                completed_states.push((key, signature.0, signature.1));
            }
            Err(error) => eprintln!("[codemux::usage_import] {error}"),
        }
    }

    // Older OpenCode versions used one JSON file per message. Scan this even
    // when SQLite exists: migrations can leave orphaned history, and the
    // global message key deduplicates rows present in both stores.
    if let Some(root) = opencode_root.as_deref() {
        let storage = root.join("storage");
        let mut session_files = Vec::new();
        find_files_with_extension(&storage.join("session"), "json", &mut session_files);
        let mut message_files = Vec::new();
        find_files_with_extension(&storage.join("message"), "json", &mut message_files);
        let mut legacy_files = session_files.clone();
        legacy_files.extend(message_files.iter().cloned());
        if let Some(signature) = tree_signature(&legacy_files) {
            let key = format!("{}::legacy-tree", storage.to_string_lossy());
            if source_changed(&db, &key, signature) {
                report.files_scanned += legacy_files.len() as i64;
                let mut child_sessions = HashSet::new();
                for path in session_files {
                    let Ok(raw) = std::fs::read_to_string(path) else {
                        continue;
                    };
                    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
                        continue;
                    };
                    if value.get("parentID").and_then(|v| v.as_str()).is_some() {
                        if let Some(id) = value.get("id").and_then(|v| v.as_str()) {
                            child_sessions.insert(id.to_string());
                        }
                    }
                }

                let mut rows = Vec::new();
                for path in message_files {
                    let Ok(raw) = std::fs::read_to_string(&path) else {
                        continue;
                    };
                    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
                        continue;
                    };
                    let message_id = value
                        .get("id")
                        .and_then(|v| v.as_str())
                        .or_else(|| path.file_stem().and_then(|v| v.to_str()))
                        .unwrap_or("unknown");
                    let session_id = value
                        .get("sessionID")
                        .and_then(|v| v.as_str())
                        .or_else(|| {
                            path.parent()
                                .and_then(|p| p.file_name())
                                .and_then(|v| v.to_str())
                        })
                        .unwrap_or("unknown");
                    if let Some(row) = parse_opencode_message(
                        &value,
                        message_id,
                        session_id,
                        signature.0,
                        child_sessions.contains(session_id),
                    ) {
                        rows.push(row);
                    }
                }
                pending_rows.extend(rows);
                completed_states.push((key, signature.0, signature.1));
            }
        }
    }

    materialize_rows(&db, pending_rows, &mut report, &mut sessions)?;
    report.sessions_found = sessions.len() as i64;
    db.set_usage_import_states(&completed_states, now_ms())?;
    // Record the version only after a complete pass so an interrupted rebuild
    // is retried instead of being accepted as current.
    if stale_version {
        db.set_setting(IMPORT_VERSION_KEY, &IMPORT_VERSION.to_string())?;
    }
    Ok(report)
}

/// Drop a cache produced by an older importer, so this scan rebuilds it.
///
/// Returns whether a purge happened, which is also the caller's signal
/// that the new version still needs recording — deliberately *after* the
/// scan completes, so a run that dies halfway is retried rather than
/// leaving a half-rebuilt ledger marked current.
///
/// An install predating the constant has no setting and is treated as
/// version 1, i.e. stale. The comparison is `!=` rather than `<` so a
/// downgrade also rebuilds: rows written by a *newer* importer are just
/// as unreadable to this one.
fn purge_if_importer_changed(db: &DatabaseStore) -> Result<bool, String> {
    let recorded = db
        .get_setting(IMPORT_VERSION_KEY)
        .and_then(|raw| raw.parse::<i64>().ok())
        .unwrap_or(1);
    if recorded == IMPORT_VERSION {
        return Ok(false);
    }
    // A failed purge propagates rather than being swallowed: continuing
    // would append new-format rows beside stale ones and double-count.
    let removed = db.reset_usage_history()?;
    eprintln!(
        "[codemux::usage_import] importer v{recorded} → v{IMPORT_VERSION}: \
         dropped {removed} cached rows, rebuilding from provider history"
    );
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(raw: &str) -> impl Iterator<Item = String> + '_ {
        raw.lines().map(|l| l.to_string())
    }

    // Redacted but structurally faithful to real logs on this machine.
    const CLAUDE_JSONL: &str = r#"
{"type":"user","sessionId":"sess-1","message":{"role":"user","content":"hi"}}
{"type":"assistant","sessionId":"sess-1","isSidechain":false,"timestamp":"2026-07-10T12:33:12.425Z","requestId":"req_1","message":{"id":"msg_A","model":"claude-fable-5","role":"assistant","usage":{"input_tokens":5595,"cache_creation_input_tokens":25661,"cache_read_input_tokens":0,"output_tokens":255,"cache_creation":{"ephemeral_1h_input_tokens":25661,"ephemeral_5m_input_tokens":0}}}}
{"type":"assistant","sessionId":"sess-1","isSidechain":false,"timestamp":"2026-07-10T12:33:13.000Z","requestId":"req_1","message":{"id":"msg_A","model":"claude-fable-5","role":"assistant","usage":{"input_tokens":5595,"cache_creation_input_tokens":25661,"cache_read_input_tokens":0,"output_tokens":812,"cache_creation":{"ephemeral_1h_input_tokens":25661,"ephemeral_5m_input_tokens":0}}}}
{"type":"worktree-state","sessionId":"sess-1"}
{"type":"assistant","sessionId":"sess-1","isSidechain":true,"timestamp":"2026-07-10T12:40:00.000Z","requestId":"req_2","message":{"id":"msg_B","model":"claude-haiku-4-5","role":"assistant","usage":{"input_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":900,"output_tokens":40}}}
"#;

    /// THE load-bearing invariant, measured from real logs: one API
    /// response is written as several records sharing a `message.id`,
    /// each carrying the same cumulative usage. Summing overcounts.
    fn find<'a>(rows: &'a [ImportedRow], message_id: &str) -> &'a ImportedRow {
        rows.iter()
            .find(|r| r.import_key.contains(message_id))
            .unwrap_or_else(|| panic!("no row for {message_id}"))
    }

    #[test]
    fn claude_repeated_message_ids_collapse_to_their_maximum() {
        let rows = parse_claude_transcript(lines(CLAUDE_JSONL));
        assert_eq!(rows.len(), 2, "two distinct message ids");
        let a = find(&rows, "msg_A");
        // NOT 255 + 812 — the records are cumulative snapshots.
        assert_eq!(a.usage.output, 812);
        assert_eq!(a.usage.input, 5595);
        assert_eq!(a.usage.cache_write, 25661);
        assert_eq!(a.model.as_deref(), Some("claude-fable-5"));
        assert!(!a.subagent);
    }

    #[test]
    fn claude_marks_sidechain_records_as_subagents() {
        let rows = parse_claude_transcript(lines(CLAUDE_JSONL));
        let b = find(&rows, "msg_B");
        assert!(b.subagent, "isSidechain drives the subagent flag");
        assert_eq!(b.usage.cache_read, 900);
    }

    /// The 1-hour tier bills at 2x input, the 5-minute tier at 1.25x, so
    /// the split has to survive the parse or the row is mispriced.
    #[test]
    fn claude_reads_the_one_hour_cache_write_subset() {
        let rows = parse_claude_transcript(lines(CLAUDE_JSONL));
        let a = find(&rows, "msg_A");
        assert_eq!(a.usage.cache_write, 25661);
        assert_eq!(a.usage.cache_write_1h, 25661, "all of it is 1-hour");
        // A record with no `cache_creation` sub-object predates the
        // split and must not claim a 1-hour share it never reported.
        let b = find(&rows, "msg_B");
        assert_eq!(b.usage.cache_write_1h, 0);
    }

    /// A response that ran several server-side iterations under-reports
    /// `cache_creation_input_tokens` relative to the per-TTL tiers. Five
    /// messages of 32,621 on this machine do this, and they account for
    /// exactly the residual gap against ccusage.
    #[test]
    fn claude_prefers_the_ephemeral_sum_when_the_total_under_reports() {
        let raw = r#"{"type":"assistant","sessionId":"s","timestamp":"2026-07-31T10:00:00.000Z","requestId":"r","message":{"id":"m","model":"claude-fable-5","usage":{"input_tokens":1,"output_tokens":2,"cache_read_input_tokens":0,"cache_creation_input_tokens":660934,"cache_creation":{"ephemeral_1h_input_tokens":700000,"ephemeral_5m_input_tokens":26864}}}}"#;
        let rows = parse_claude_transcript(lines(raw));
        assert_eq!(rows[0].usage.cache_write, 726_864, "the larger of the two");
        assert_eq!(rows[0].usage.cache_write_1h, 700_000);
    }

    /// `<synthetic>` records are CLI-generated placeholders that were
    /// never billed; an accounting ledger must not carry them.
    #[test]
    fn claude_skips_synthetic_records() {
        let raw = r#"{"type":"assistant","sessionId":"s","timestamp":"2026-07-31T10:00:00.000Z","message":{"id":"m","model":"<synthetic>","usage":{"input_tokens":5,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
        let rows = parse_claude_transcript(lines(raw));
        assert!(rows.is_empty());
    }

    /// Codemux-launched sessions are present in the same provider history as
    /// terminal-launched sessions and must be counted exactly once there.
    #[test]
    fn claude_includes_sessions_regardless_of_launcher() {
        let rows = parse_claude_transcript(lines(CLAUDE_JSONL));
        assert_eq!(rows.len(), 2);
    }

    /// The key is ccusage's — `message.id` + `requestId` — and crucially
    /// NOT session-scoped, so the same response copied into a resumed or
    /// forked transcript dedups against the unique index.
    #[test]
    fn claude_import_keys_are_global_and_carry_the_request_id() {
        let rows = parse_claude_transcript(lines(CLAUDE_JSONL));
        let keys: HashSet<&str> = rows.iter().map(|r| r.import_key.as_str()).collect();
        assert_eq!(keys.len(), rows.len());
        assert!(keys.contains("claude:msg_A:req_1"), "got {keys:?}");
        assert!(!keys.iter().any(|k| k.contains("sess-1")));

        // The same response under a different session id produces the
        // SAME key, which is the whole point.
        let forked = CLAUDE_JSONL.replace("sess-1", "sess-2");
        let forked_rows = parse_claude_transcript(lines(&forked));
        let forked_keys: HashSet<&str> =
            forked_rows.iter().map(|r| r.import_key.as_str()).collect();
        assert_eq!(keys, forked_keys);
    }

    #[test]
    fn claude_rows_without_a_request_id_still_get_a_stable_key() {
        let raw = r#"{"type":"assistant","sessionId":"s","timestamp":"2026-07-31T10:00:00.000Z","message":{"id":"m","model":"claude-fable-5","usage":{"input_tokens":5,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
        let rows = parse_claude_transcript(lines(raw));
        assert_eq!(rows[0].import_key, "claude:m:-");
    }

    /// Logs are appended live, so the tail can be half-written.
    #[test]
    fn malformed_lines_never_abort_the_file() {
        let raw = format!(
            "{}\n{{\"type\":\"assistant\",\"broken\n\nnot json at all\n",
            CLAUDE_JSONL.trim()
        );
        let rows = parse_claude_transcript(lines(&raw));
        assert_eq!(rows.len(), 2, "good records still land");
    }

    #[test]
    fn claude_transcript_without_usage_yields_nothing() {
        let raw = r#"{"type":"user","sessionId":"s"}
{"type":"summary"}"#;
        let rows = parse_claude_transcript(lines(raw));
        assert!(rows.is_empty());
    }

    /// Two turns a day apart, with a mid-rollout model switch — the two
    /// things the old one-row-per-rollout parse got wrong.
    const CODEX_JSONL: &str = r#"
{"timestamp":"2026-07-21T13:17:45.999Z","type":"session_meta","payload":{"id":"019f84d2-5235-7aa1-98dc-3ac160d00a32","cwd":"/home/z/p"}}
{"timestamp":"2026-07-21T13:17:46.000Z","type":"turn_context","payload":{"turn_id":"t1","model":"gpt-5.6-sol","effort":"high"}}
{"timestamp":"2026-07-21T13:17:53.831Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":13826,"cached_input_tokens":9984,"cache_write_input_tokens":0,"output_tokens":345,"reasoning_output_tokens":88,"total_tokens":14171},"last_token_usage":{"input_tokens":13826,"cached_input_tokens":9984,"cache_write_input_tokens":0,"output_tokens":345,"reasoning_output_tokens":88,"total_tokens":14171}}}}
{"timestamp":"2026-07-22T16:56:00.000Z","type":"turn_context","payload":{"turn_id":"t2","model":"gpt-5.6-luna","effort":"low"}}
{"timestamp":"2026-07-22T16:56:17.845Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":51029868,"cached_input_tokens":49715968,"cache_write_input_tokens":0,"output_tokens":69405,"reasoning_output_tokens":26409,"total_tokens":51099273},"last_token_usage":{"input_tokens":51016042,"cached_input_tokens":49705984,"cache_write_input_tokens":0,"output_tokens":69060,"reasoning_output_tokens":26321,"total_tokens":51085102}}}}
"#;

    /// The per-turn grain: one row per `token_count`, stamped with that
    /// turn's own timestamp and priced at the model that served it.
    #[test]
    fn codex_imports_one_row_per_turn_from_the_delta_counter() {
        let rows = parse_codex_rollout(lines(CODEX_JSONL));
        assert_eq!(rows.len(), 2, "one row per token_count event");

        let first = &rows[0];
        assert_eq!(first.usage.cache_read, 9_984);
        assert_eq!(first.usage.input, 13_826 - 9_984, "fresh input only");
        assert_eq!(first.usage.output, 345);
        assert_eq!(first.usage.reasoning, 88);
        assert_eq!(first.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(
            first.import_key,
            "codex:019f84d2-5235-7aa1-98dc-3ac160d00a32:0"
        );

        let second = &rows[1];
        // The DELTA, not the cumulative total.
        assert_eq!(second.usage.output, 69_060);
        assert_eq!(second.usage.cache_read, 49_705_984);
        // A mid-rollout model switch is respected rather than smeared.
        assert_eq!(second.model.as_deref(), Some("gpt-5.6-luna"));
        // …and it lands on its own day rather than the rollout's start.
        assert!(
            second.created_at_ms - first.created_at_ms > 24 * 60 * 60 * 1000,
            "each turn keeps its own timestamp"
        );

        // The deltas reconcile with the final cumulative total.
        assert_eq!(
            first.usage.output + second.usage.output,
            69_405,
            "sum of deltas == final total_token_usage"
        );
    }

    /// An identical `last_token_usage` in two adjacent events is a
    /// re-send, not a second turn that coincided to the token.
    #[test]
    fn codex_suppresses_consecutive_duplicate_deltas() {
        let repeated = r#"
{"timestamp":"2026-07-21T13:17:45.999Z","type":"session_meta","payload":{"id":"roll-1"}}
{"timestamp":"2026-07-21T13:17:46.000Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}
{"timestamp":"2026-07-21T13:17:53.000Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":10,"total_tokens":110}}}}
{"timestamp":"2026-07-21T13:17:54.000Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":10,"total_tokens":110}}}}
{"timestamp":"2026-07-21T13:17:55.000Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":200,"cached_input_tokens":0,"output_tokens":20,"total_tokens":220}}}}
"#;
        let rows = parse_codex_rollout(lines(repeated));
        assert_eq!(rows.len(), 2, "the repeat is dropped");
        assert_eq!(rows[0].usage.output, 10);
        assert_eq!(rows[1].usage.output, 20);
    }

    /// `cache_write_input_tokens` IS in the local format (an earlier
    /// version of this file claimed it was not) and is carved out of
    /// `input_tokens` alongside the cached count.
    #[test]
    fn codex_reads_the_cache_write_counter_and_carves_it_out() {
        let raw = r#"
{"timestamp":"2026-07-21T13:17:45.999Z","type":"session_meta","payload":{"id":"roll-2"}}
{"timestamp":"2026-07-21T13:17:46.000Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}
{"timestamp":"2026-07-21T13:17:53.000Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1000,"cached_input_tokens":600,"cache_write_input_tokens":300,"output_tokens":50,"total_tokens":1050}}}}
"#;
        let rows = parse_codex_rollout(lines(raw));
        assert_eq!(rows[0].usage.cache_write, 300);
        assert_eq!(rows[0].usage.cache_read, 600);
        assert_eq!(rows[0].usage.input, 100, "1000 - 600 cached - 300 written");
    }

    /// Rollouts predating `last_token_usage` (2 of 148 files here) must
    /// still import, via the old cumulative-aggregate path.
    #[test]
    fn codex_falls_back_to_one_aggregate_row_for_legacy_rollouts() {
        let legacy = r#"
{"timestamp":"2026-07-21T13:17:45.999Z","type":"session_meta","payload":{"id":"roll-3"}}
{"timestamp":"2026-07-21T13:17:46.000Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}
{"timestamp":"2026-07-21T13:17:53.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":10,"total_tokens":110}}}}
{"timestamp":"2026-07-21T13:17:59.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":300,"cached_input_tokens":40,"output_tokens":25,"total_tokens":325}}}}
{"timestamp":"2026-07-21T13:18:00.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":5,"cached_input_tokens":0,"output_tokens":1,"total_tokens":6}}}}
"#;
        let rows = parse_codex_rollout(lines(legacy));
        assert_eq!(rows.len(), 1, "one aggregate row per legacy rollout");
        // A truncated tail must not shrink an already-observed total.
        assert_eq!(rows[0].usage.output, 25, "kept the larger reading");
        assert_eq!(rows[0].import_key, "codex:roll-3");
    }

    #[test]
    fn codex_includes_rollouts_regardless_of_launcher() {
        let rows = parse_codex_rollout(lines(CODEX_JSONL));
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn codex_marks_child_rollouts_as_subagents() {
        let raw = r#"
{"timestamp":"2026-07-21T13:17:45.999Z","type":"session_meta","payload":{"id":"child-1","source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent-1","depth":1}}},"parent_thread_id":"parent-1"}}
{"timestamp":"2026-07-21T13:17:46.000Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}
{"timestamp":"2026-07-21T13:17:53.000Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":10,"total_tokens":110}}}}
"#;
        let rows = parse_codex_rollout(lines(raw));
        assert_eq!(rows.len(), 1);
        assert!(rows[0].subagent);
    }

    #[test]
    fn codex_rollout_without_token_counts_yields_nothing() {
        let raw = r#"{"timestamp":"2026-07-21T13:17:45.999Z","type":"session_meta","payload":{"id":"abc"}}"#;
        let rows = parse_codex_rollout(lines(raw));
        assert!(rows.is_empty());
    }

    #[test]
    fn opencode_normalizes_assistant_messages_and_preserves_reported_cost() {
        let data = serde_json::json!({
            "id": "msg-1",
            "sessionID": "ses-1",
            "role": "assistant",
            "providerID": "anthropic",
            "modelID": "claude-sonnet-4-6",
            "cost": 1.25,
            "tokens": {
                "input": 100,
                "output": 40,
                "reasoning": 10,
                "cache": {"read": 700, "write": 20}
            },
            "time": {"created": 1_800_000_000_000_i64}
        });
        let row = parse_opencode_message(&data, "fallback", "ses-1", 1, true).unwrap();
        assert_eq!(row.import_key, "opencode:msg-1");
        assert_eq!(row.model.as_deref(), Some("anthropic/claude-sonnet-4-6"));
        assert_eq!(row.usage.input, 100);
        assert_eq!(row.usage.output, 50, "plain output + reasoning");
        assert_eq!(row.usage.reasoning, 10);
        assert_eq!(row.usage.cache_read, 700);
        assert_eq!(row.usage.cache_write, 20);
        assert_eq!(row.reported_cost_usd, Some(1.25));
        assert!(row.subagent);
    }

    #[test]
    fn opencode_ignores_user_and_tokenless_messages() {
        let user = serde_json::json!({"role": "user", "tokens": {"input": 10}});
        assert!(parse_opencode_message(&user, "m", "s", 1, false).is_none());
        let empty = serde_json::json!({
            "role": "assistant",
            "tokens": {"input": 0, "output": 0, "reasoning": 0,
                       "cache": {"read": 0, "write": 0}}
        });
        assert!(parse_opencode_message(&empty, "m", "s", 1, false).is_none());
    }

    #[test]
    fn opencode_sqlite_reader_uses_messages_and_parent_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("opencode.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT);
             CREATE TABLE message (
                 id TEXT PRIMARY KEY,
                 session_id TEXT NOT NULL,
                 time_created INTEGER NOT NULL,
                 data TEXT NOT NULL
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, parent_id) VALUES ('child', 'parent')",
            [],
        )
        .unwrap();
        let data = serde_json::json!({
            "role": "assistant",
            "providerID": "openai",
            "modelID": "gpt-5",
            "cost": 0.75,
            "tokens": {"input": 12, "output": 4, "reasoning": 1,
                       "cache": {"read": 8, "write": 0}}
        })
        .to_string();
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, data)
             VALUES ('msg-db', 'child', 1800000000000, ?1)",
            [data],
        )
        .unwrap();
        drop(conn);

        let rows = read_opencode_db(&path).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].import_key, "opencode:msg-db");
        assert!(rows[0].subagent);
        assert_eq!(rows[0].usage.output, 5);
        assert_eq!(rows[0].reported_cost_usd, Some(0.75));
    }

    // ── importer versioning ──

    fn seeded_db() -> DatabaseStore {
        let db = DatabaseStore::new_in_memory();
        db.upsert_provider_usage_row(
            "claude:msg_A:req_1",
            1_800_000_000_000,
            "cli:sess-1",
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
        db.set_usage_import_state("/logs/a.jsonl", 1, 2, 2, 3)
            .unwrap();
        db
    }

    /// An install predating the version constant carries rows priced by
    /// the old table (`gpt-5.6-sol` at a quarter of its real rate) and
    /// keyed by the old `import_key`, so a version mismatch rebuilds the
    /// materialized cache from provider history.
    #[test]
    fn a_stale_importer_version_purges_and_rebuilds() {
        let db = seeded_db();
        assert_eq!(db.usage_rows_since(0).unwrap().len(), 1);

        assert!(
            purge_if_importer_changed(&db).unwrap(),
            "version 1 is stale"
        );
        assert!(db.usage_rows_since(0).unwrap().is_empty());
        assert!(
            db.usage_import_state("/logs/a.jsonl").unwrap().is_none(),
            "file state goes too, or the rebuild scan skips everything"
        );
    }

    /// The purge must not repeat once the version is recorded, or every
    /// page open would rebuild the whole ledger.
    #[test]
    fn a_current_importer_version_leaves_the_ledger_alone() {
        let db = seeded_db();
        db.set_setting(IMPORT_VERSION_KEY, &IMPORT_VERSION.to_string())
            .unwrap();

        assert!(!purge_if_importer_changed(&db).unwrap());
        assert_eq!(db.usage_rows_since(0).unwrap().len(), 1, "rows survive");
        assert!(db.usage_import_state("/logs/a.jsonl").unwrap().is_some());
    }

    /// A downgrade is as stale as an upgrade: rows written by a newer
    /// importer are just as unreadable to this one.
    #[test]
    fn a_newer_recorded_version_also_triggers_a_rebuild() {
        let db = seeded_db();
        db.set_setting(IMPORT_VERSION_KEY, &(IMPORT_VERSION + 5).to_string())
            .unwrap();
        assert!(purge_if_importer_changed(&db).unwrap());
        assert!(db.usage_rows_since(0).unwrap().is_empty());
    }

    #[test]
    fn iso8601_parses_to_epoch_millis() {
        assert_eq!(parse_iso8601_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(parse_iso8601_ms("1970-01-01T00:00:01.500Z"), Some(1500));
        // Spot-checked against `date -u -d ... +%s`.
        assert_eq!(
            parse_iso8601_ms("2026-07-10T12:33:12.425Z"),
            Some(1_783_686_792_425)
        );
        assert!(parse_iso8601_ms("not a date").is_none());
        assert!(parse_iso8601_ms("").is_none());
    }
}
