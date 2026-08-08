//! Import token usage from the coding CLIs' own local logs.
//!
//! A PTY tab streams no runtime events, so a terminal-launched
//! `claude` or `codex` session is invisible to the live ledger. Both
//! CLIs, however, write a complete JSONL transcript to disk, and those
//! transcripts carry the same usage blocks the SDK reports. Reading them
//! is the honest maximum: no provider exposes subscription history over
//! an API, so **this machine's logs are the only complete record that
//! exists**, and only for the machine the app is running on.
//!
//! Two hazards shape the design:
//!
//! 1. **Codemux's own sessions write these logs too.** Every
//!    Codemux-driven turn already has a live ledger row, so an import
//!    that did not exclude them would double-count the entire history
//!    the moment the toggle is switched on. Sessions whose id matches a
//!    persisted `sdk_session_id` are skipped — see [`ScanContext`].
//! 2. **Re-scanning must be idempotent.** Every imported row carries an
//!    `import_key`, unique-indexed, and insertion is `INSERT OR IGNORE`.
//!    A second scan of an unchanged file adds nothing even if the
//!    offset bookkeeping is lost.
//!
//! Scans are incremental: `usage_import_state` remembers each file's
//! size and how much was consumed, so a warm re-scan reads only what
//! grew.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::agent_provider::pricing;
use crate::database::DatabaseStore;

/// Bump this whenever a change would make previously-imported rows
/// wrong — a parse fix, a new field, a price correction.
///
/// Imported rows freeze `cost_usd` at insert and are keyed by
/// `import_key`, so without a version gate a corrected importer would
/// leave every existing install showing figures produced by the old,
/// wrong code forever: `INSERT OR IGNORE` sees the same keys and does
/// nothing. On a mismatch the scan drops every `source = 'cli_import'`
/// row, clears the per-file state, and rebuilds from the logs, which are
/// still on disk. Live rows are never touched.
///
/// | v | why |
/// |---|---|
/// | 1 | initial import (implicit — installs predating this constant) |
/// | 2 | gpt-5.6-sol/-terra/-luna priced specifically instead of falling through to `gpt-5` (a 4x under-report); Claude 1-hour cache writes billed at 2x input; Codex imported per turn instead of one aggregate per rollout; Codex `cache_write_input_tokens` read; Claude `import_key` moved to ccusage's global `message.id`+`requestId` |
pub const IMPORT_VERSION: i64 = 2;

/// Settings key holding the [`IMPORT_VERSION`] the current rows were
/// produced by.
const IMPORT_VERSION_KEY: &str = "usage.import_version";

/// What one scan did, for the footer note.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CliImportReport {
    pub files_scanned: i64,
    pub sessions_found: i64,
    pub rows_added: i64,
    /// Sessions skipped because Codemux itself drove them.
    pub sessions_skipped_own: i64,
    /// True when this scan found a stale [`IMPORT_VERSION`] and rebuilt
    /// the imported half of the ledger from scratch.
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
}

/// Everything a scan needs that is not the file itself.
pub struct ScanContext {
    /// Provider-native session ids Codemux already has live rows for.
    pub own_session_ids: HashSet<String>,
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
pub fn parse_claude_transcript(
    lines: impl Iterator<Item = String>,
    ctx: &ScanContext,
) -> (Vec<ImportedRow>, bool) {
    // message.id -> (usage high-water, first timestamp, model, subagent,
    //                requestId)
    let mut by_message: HashMap<String, ClaudeMessageAcc> = HashMap::new();
    let mut session_id: Option<String> = None;
    let mut skipped_own = false;

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
            if ctx.own_session_ids.contains(sid) {
                // Codemux drove this session; the live ledger already has it.
                return (Vec::new(), true);
            }
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
        return (Vec::new(), skipped_own);
    };
    skipped_own = false;

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
        })
        .collect();
    rows.sort_by_key(|r| r.created_at_ms);
    (rows, skipped_own)
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
pub fn parse_codex_rollout(
    lines: impl Iterator<Item = String>,
    ctx: &ScanContext,
) -> (Vec<ImportedRow>, bool) {
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
                    if ctx.own_session_ids.contains(id) {
                        return (Vec::new(), true);
                    }
                    session_id.get_or_insert_with(|| id.to_string());
                }
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
                        // A rollout is one agent's transcript; subagent
                        // attribution is not recoverable from it.
                        subagent: false,
                        usage: delta,
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
        return (Vec::new(), false);
    };
    if saw_delta {
        return (rows, false);
    }
    if best.is_empty() {
        return (Vec::new(), false);
    }
    let row = ImportedRow {
        // One row per rollout, so the rollout id alone is a stable key.
        import_key: format!("codex:{session_id}"),
        created_at_ms: if created_at_ms > 0 { created_at_ms } else { last_ts },
        session_id,
        provider: "codex",
        model,
        subagent: false,
        usage: best,
    };
    (vec![row], false)
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

/// Scan both CLIs' logs and import anything new.
///
/// Incremental by file size: a file whose size is unchanged since the
/// last scan is skipped outright. A file that grew is re-read **in
/// full** rather than from the stored offset, because both formats need
/// whole-file context — Claude's per-message maxima and Codex's session
/// meta both live earlier in the file than the tail. The offset is
/// still recorded so an unchanged file costs a single `stat`, and the
/// `import_key` index makes the re-read free of duplicates.
#[tauri::command]
pub async fn usage_scan_cli_logs(
    db: State<'_, DatabaseStore>,
) -> Result<CliImportReport, String> {
    let home = dirs::home_dir().ok_or("Could not determine home directory")?;
    let own_session_ids = db.known_sdk_session_ids().unwrap_or_default();
    let ctx = ScanContext { own_session_ids };
    let mut report = CliImportReport::default();

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
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            let size = meta.len() as i64;
            let mtime_ms = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let key = path.to_string_lossy().to_string();
            if let Ok(Some((_, prev_size, _))) = db.usage_import_state(&key) {
                if prev_size == size {
                    // Nothing appended since the last scan.
                    continue;
                }
            }
            report.files_scanned += 1;

            let Ok(file) = std::fs::File::open(&path) else {
                continue;
            };
            let mut reader = BufReader::new(file);
            let _ = reader.seek(SeekFrom::Start(0));
            let lines = reader.lines().map_while(Result::ok);

            let (rows, skipped_own) = match provider {
                "claude" => parse_claude_transcript(lines, &ctx),
                _ => parse_codex_rollout(lines, &ctx),
            };
            if skipped_own {
                report.sessions_skipped_own += 1;
            } else if !rows.is_empty() {
                report.sessions_found += 1;
            }
            for row in rows {
                let cost = pricing::cost_for_with_1h(
                    row.model.as_deref(),
                    row.usage.input.max(0) as u64,
                    row.usage.output.max(0) as u64,
                    row.usage.cache_read.max(0) as u64,
                    row.usage.cache_write.max(0) as u64,
                    row.usage.cache_write_1h.max(0) as u64,
                );
                match db.insert_imported_usage_row(
                    &row.import_key,
                    row.created_at_ms,
                    &format!("cli:{}", row.session_id),
                    row.provider,
                    row.model.as_deref(),
                    row.subagent,
                    row.usage.input,
                    row.usage.output,
                    row.usage.cache_read,
                    row.usage.cache_write,
                    row.usage.reasoning,
                    cost,
                ) {
                    Ok(true) => report.rows_added += 1,
                    Ok(false) => {}
                    Err(error) => {
                        eprintln!("[codemux::usage_import] insert failed: {error}");
                    }
                }
            }
            let _ = db.set_usage_import_state(&key, mtime_ms, size, size, now_ms());
        }
    }

    // Only after a full successful pass — recording it earlier would let
    // a scan that died halfway leave a half-rebuilt ledger looking
    // current.
    if stale_version {
        let _ = db.set_setting(IMPORT_VERSION_KEY, &IMPORT_VERSION.to_string());
    }

    Ok(report)
}

/// Drop imported rows produced by an older importer, so this scan
/// rebuilds them.
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
    let removed = db.reset_cli_imports()?;
    eprintln!(
        "[codemux::usage_import] importer v{recorded} → v{IMPORT_VERSION}: \
         dropped {removed} imported rows, re-importing from the logs"
    );
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(own: &[&str]) -> ScanContext {
        ScanContext {
            own_session_ids: own.iter().map(|s| s.to_string()).collect(),
        }
    }

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
        let (rows, _) = parse_claude_transcript(lines(CLAUDE_JSONL), &ctx(&[]));
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
        let (rows, _) = parse_claude_transcript(lines(CLAUDE_JSONL), &ctx(&[]));
        let b = find(&rows, "msg_B");
        assert!(b.subagent, "isSidechain drives the subagent flag");
        assert_eq!(b.usage.cache_read, 900);
    }

    /// The 1-hour tier bills at 2x input, the 5-minute tier at 1.25x, so
    /// the split has to survive the parse or the row is mispriced.
    #[test]
    fn claude_reads_the_one_hour_cache_write_subset() {
        let (rows, _) = parse_claude_transcript(lines(CLAUDE_JSONL), &ctx(&[]));
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
        let (rows, _) = parse_claude_transcript(lines(raw), &ctx(&[]));
        assert_eq!(rows[0].usage.cache_write, 726_864, "the larger of the two");
        assert_eq!(rows[0].usage.cache_write_1h, 700_000);
    }

    /// `<synthetic>` records are CLI-generated placeholders that were
    /// never billed; an accounting ledger must not carry them.
    #[test]
    fn claude_skips_synthetic_records() {
        let raw = r#"{"type":"assistant","sessionId":"s","timestamp":"2026-07-31T10:00:00.000Z","message":{"id":"m","model":"<synthetic>","usage":{"input_tokens":5,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
        let (rows, _) = parse_claude_transcript(lines(raw), &ctx(&[]));
        assert!(rows.is_empty());
    }

    /// Without this the whole live ledger double-counts the instant the
    /// toggle is switched on.
    #[test]
    fn claude_skips_sessions_codemux_itself_drove() {
        let (rows, skipped) = parse_claude_transcript(lines(CLAUDE_JSONL), &ctx(&["sess-1"]));
        assert!(rows.is_empty());
        assert!(skipped, "and reports why");
    }

    /// The key is ccusage's — `message.id` + `requestId` — and crucially
    /// NOT session-scoped, so the same response copied into a resumed or
    /// forked transcript dedups against the unique index.
    #[test]
    fn claude_import_keys_are_global_and_carry_the_request_id() {
        let (rows, _) = parse_claude_transcript(lines(CLAUDE_JSONL), &ctx(&[]));
        let keys: HashSet<&str> = rows.iter().map(|r| r.import_key.as_str()).collect();
        assert_eq!(keys.len(), rows.len());
        assert!(keys.contains("claude:msg_A:req_1"), "got {keys:?}");
        assert!(!keys.iter().any(|k| k.contains("sess-1")));

        // The same response under a different session id produces the
        // SAME key, which is the whole point.
        let forked = CLAUDE_JSONL.replace("sess-1", "sess-2");
        let (forked_rows, _) = parse_claude_transcript(lines(&forked), &ctx(&[]));
        let forked_keys: HashSet<&str> =
            forked_rows.iter().map(|r| r.import_key.as_str()).collect();
        assert_eq!(keys, forked_keys);
    }

    #[test]
    fn claude_rows_without_a_request_id_still_get_a_stable_key() {
        let raw = r#"{"type":"assistant","sessionId":"s","timestamp":"2026-07-31T10:00:00.000Z","message":{"id":"m","model":"claude-fable-5","usage":{"input_tokens":5,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#;
        let (rows, _) = parse_claude_transcript(lines(raw), &ctx(&[]));
        assert_eq!(rows[0].import_key, "claude:m:-");
    }

    /// Logs are appended live, so the tail can be half-written.
    #[test]
    fn malformed_lines_never_abort_the_file() {
        let raw = format!(
            "{}\n{{\"type\":\"assistant\",\"broken\n\nnot json at all\n",
            CLAUDE_JSONL.trim()
        );
        let (rows, _) = parse_claude_transcript(lines(&raw), &ctx(&[]));
        assert_eq!(rows.len(), 2, "good records still land");
    }

    #[test]
    fn claude_transcript_without_usage_yields_nothing() {
        let raw = r#"{"type":"user","sessionId":"s"}
{"type":"summary"}"#;
        let (rows, _) = parse_claude_transcript(lines(raw), &ctx(&[]));
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
        let (rows, _) = parse_codex_rollout(lines(CODEX_JSONL), &ctx(&[]));
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
        let (rows, _) = parse_codex_rollout(lines(repeated), &ctx(&[]));
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
        let (rows, _) = parse_codex_rollout(lines(raw), &ctx(&[]));
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
        let (rows, _) = parse_codex_rollout(lines(legacy), &ctx(&[]));
        assert_eq!(rows.len(), 1, "one aggregate row per legacy rollout");
        // A truncated tail must not shrink an already-observed total.
        assert_eq!(rows[0].usage.output, 25, "kept the larger reading");
        assert_eq!(rows[0].import_key, "codex:roll-3");
    }

    #[test]
    fn codex_skips_rollouts_codemux_itself_drove() {
        let (rows, skipped) = parse_codex_rollout(
            lines(CODEX_JSONL),
            &ctx(&["019f84d2-5235-7aa1-98dc-3ac160d00a32"]),
        );
        assert!(rows.is_empty());
        assert!(skipped);
    }

    #[test]
    fn codex_rollout_without_token_counts_yields_nothing() {
        let raw = r#"{"timestamp":"2026-07-21T13:17:45.999Z","type":"session_meta","payload":{"id":"abc"}}"#;
        let (rows, _) = parse_codex_rollout(lines(raw), &ctx(&[]));
        assert!(rows.is_empty());
    }

    // ── importer versioning ──

    fn seeded_db() -> DatabaseStore {
        let db = DatabaseStore::new_in_memory();
        db.insert_imported_usage_row(
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
        )
        .unwrap();
        db.set_usage_import_state("/logs/a.jsonl", 1, 2, 2, 3).unwrap();
        db
    }

    /// An install predating the version constant carries rows priced by
    /// the old table (`gpt-5.6-sol` at a quarter of its real rate) and
    /// keyed by the old `import_key`. Because insertion is
    /// `INSERT OR IGNORE`, a plain re-scan would change nothing — so a
    /// version mismatch has to purge first.
    #[test]
    fn a_stale_importer_version_purges_and_rebuilds() {
        let db = seeded_db();
        assert_eq!(db.usage_rows_since(0).unwrap().len(), 1);

        assert!(purge_if_importer_changed(&db).unwrap(), "version 1 is stale");
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
