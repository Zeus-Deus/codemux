//! Read side of the usage ledger — the query surface behind
//! Settings → Usage.
//!
//! The ledger is a materialized cache of provider-owned local history; this
//! module only reads and aggregates it.
//! Aggregation is a pure function over the rows so it can be unit-tested
//! without a database, and so the three views the dashboard needs
//! (time buckets, provider lanes, headline totals) are guaranteed to be
//! three slices of exactly the same data rather than three queries that
//! have to agree by hand.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::agent_provider::{PlanAuthMode, PlanUsageWindow};
use crate::database::{DatabaseStore, UsageLedgerRow};

/// Live plan-quota readings, keyed by provider.
///
/// Deliberately in-memory and **not** persisted. A quota reading is a
/// level whose meaning decays in minutes: a snapshot replayed from disk
/// after a restart would show confidently wrong bars, which is worse
/// than showing none. The store repopulates as soon as a session runs,
/// and the dashboard renders no meters for a provider it has not heard
/// from.
#[derive(Default)]
pub struct PlanQuotaStore {
    inner: Mutex<HashMap<String, ProviderQuota>>,
}

/// One provider's newest plan-quota snapshot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderQuota {
    pub windows: Vec<PlanUsageWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_mode: Option<PlanAuthMode>,
    /// When this snapshot landed, unix ms — so the UI can age it out.
    pub received_at_ms: i64,
}

impl PlanQuotaStore {
    /// Merge one `PlanUsageUpdated` into the store.
    ///
    /// Fields are merged rather than replaced wholesale because the two
    /// halves arrive separately: Codex reports plan/auth from
    /// `account/read` at init (with no windows) and the windows from
    /// `account/rateLimits/*` (with no auth mode). A blind overwrite
    /// would let each half erase the other's contribution.
    ///
    /// Within a field the newest value wins — an empty `windows` list is
    /// treated as "nothing to say", not as "no quota".
    pub fn record(
        &self,
        provider: &str,
        windows: Vec<PlanUsageWindow>,
        plan_label: Option<String>,
        auth_mode: Option<PlanAuthMode>,
        received_at_ms: i64,
    ) {
        let Ok(mut map) = self.inner.lock() else {
            return;
        };
        let entry = map.entry(provider.to_string()).or_default();
        if !windows.is_empty() {
            entry.windows = windows;
        }
        if plan_label.is_some() {
            entry.plan_label = plan_label;
        }
        if auth_mode.is_some() {
            entry.auth_mode = auth_mode;
        }
        entry.received_at_ms = received_at_ms;
    }

    /// Snapshot of everything known, for the dashboard query.
    pub fn snapshot(&self) -> HashMap<String, ProviderQuota> {
        self.inner
            .lock()
            .map(|map| map.clone())
            .unwrap_or_default()
    }

    /// The detected billing mode for `provider`, when one is known.
    pub fn auth_mode_for(&self, provider: &str) -> Option<PlanAuthMode> {
        self.inner
            .lock()
            .ok()
            .and_then(|map| map.get(provider).and_then(|q| q.auth_mode))
    }
}

/// One bar in the overview chart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageBucket {
    /// Bucket start, unix milliseconds.
    pub start_ms: i64,
    /// Short axis label (`"14:00"` hourly, `"8"` daily).
    pub label: String,
    /// Full label for the hover readout (`"Today 14:00"`, `"Aug 8"`).
    pub sub_label: String,
    /// Per-provider slice of this bucket, provider id → figures.
    pub providers: HashMap<String, BucketSlice>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BucketSlice {
    pub tokens: i64,
    pub cost_usd: f64,
}

/// One model's contribution inside a provider lane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUsage {
    pub model: String,
    pub tokens: i64,
    pub cost_usd: f64,
    /// Tokens attributable to subagent work. `0` when the model never
    /// ran as a subagent; the lane renders a "· subagents" note only
    /// when this is positive.
    pub subagent_tokens: i64,
}

/// One provider lane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderUsage {
    pub provider: String,
    pub tokens: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cost_usd: f64,
    /// Distinct threads that produced any usage in range.
    pub session_count: i64,
    /// Per-model rows, largest first.
    pub models: Vec<ModelUsage>,
}

/// Headline totals for the selected period.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageTotals {
    /// API/list-price equivalent. This is an estimate of the work's token
    /// value, never a claim about what the user was actually charged.
    pub estimated_cost_usd: f64,
    pub total_tokens: i64,
    /// Share of `total_tokens` served from cache reads, 0.0–1.0.
    pub cache_read_share: f64,
    pub session_count: i64,
}

/// The token-composition strip: where the period's tokens went.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageComposition {
    /// Every token the providers processed — the four-way sum.
    pub processed_tokens: i64,
    pub cache_read_tokens: i64,
    /// Cache reads as a share of *observed input* (input + cache read +
    /// cache write), 0–1. Input is the only meaningful denominator: a
    /// cache hit replaces fresh input, never output.
    pub cache_read_share_of_input: f64,
    pub input_tokens: i64,
    pub cache_write_tokens: i64,
    pub output_tokens: i64,
    /// Subset of `output_tokens`; `0` when no provider in range reported
    /// a split, which is how the UI decides to hide the sub-note.
    pub reasoning_tokens: i64,
    /// Estimated dollars saved by caching versus paying full input rate
    /// for every prompt token.
    pub cache_savings_usd: f64,
    /// `raw / actual` — how many times the bill would have been without
    /// caching. `None` when actual spend rounds to nothing, or when no
    /// model in range could be priced.
    pub cache_savings_multiplier: Option<f64>,
}

/// How trustworthy the period's cost figures are.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CostConfidence {
    /// Share of total cost from rows the provider itself priced, 0–1.
    pub provider_reported_share: f64,
    /// Share of total cost priced from the static table, 0–1.
    pub table_priced_share: f64,
    /// Share of **tokens** from rows with no price at all, 0–1.
    ///
    /// Tokens rather than cost on purpose: an unpriced row contributes
    /// zero cost, so a cost-share would always read 0% and hide exactly
    /// the thing this row exists to surface.
    pub unpriced_token_share: f64,
    pub cache_savings_usd: f64,
}

/// One row of the flat, cross-provider model breakdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlatModelUsage {
    pub provider: String,
    pub model: String,
    pub tokens: i64,
    pub cost_usd: f64,
    /// `false` when every row for this model was unpriced — the UI shows
    /// an em-dash and falls back to a token share.
    pub priced: bool,
    /// True when any of this model's cost came from the provider's own
    /// catalogue rather than the static table.
    pub provider_reported: bool,
}

/// Everything one Settings → Usage render needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageSummary {
    pub period: String,
    /// Inclusive lower bound of the range, unix ms.
    pub start_ms: i64,
    /// Exclusive upper bound (i.e. "now"), unix ms.
    pub end_ms: i64,
    pub buckets: Vec<UsageBucket>,
    pub providers: Vec<ProviderUsage>,
    pub totals: UsageTotals,
    pub composition: UsageComposition,
    pub confidence: CostConfidence,
    /// Flat cross-provider model breakdown, most expensive first.
    pub models: Vec<FlatModelUsage>,
    /// Live plan-quota readings, keyed by provider id. Absent providers
    /// simply have no meters — the dashboard renders nothing rather than
    /// an empty bar.
    pub quota: HashMap<String, ProviderQuota>,
    /// When the figures were computed. The ledger is local, so this is
    /// always "now" — the field exists so the UI can say so honestly
    /// rather than implying a remote sync that does not happen.
    pub synced_at_ms: i64,
}

/// The three ranges the period control offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Period {
    Today,
    Days7,
    Days30,
    Days90,
}

impl Period {
    fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "today" => Ok(Self::Today),
            "7d" => Ok(Self::Days7),
            "30d" => Ok(Self::Days30),
            "90d" => Ok(Self::Days90),
            other => Err(format!("unknown period: {other}")),
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::Today => "today",
            Self::Days7 => "7d",
            Self::Days30 => "30d",
            Self::Days90 => "90d",
        }
    }

    /// Bucket width in milliseconds: hourly for today, daily otherwise.
    fn bucket_ms(&self) -> i64 {
        match self {
            Self::Today => HOUR_MS,
            _ => DAY_MS,
        }
    }

    fn bucket_count(&self) -> i64 {
        match self {
            Self::Today => 24,
            Self::Days7 => 7,
            Self::Days30 => 30,
            Self::Days90 => 90,
        }
    }
}

const HOUR_MS: i64 = 60 * 60 * 1000;
const DAY_MS: i64 = 24 * HOUR_MS;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Aggregate `rows` into the dashboard's shape.
///
/// Pure: `now_ms` is a parameter so tests can pin the window rather than
/// racing the clock. Rows outside `[start_ms, now_ms)` are ignored, so
/// the caller can over-fetch safely.
fn summarize(
    rows: &[UsageLedgerRow],
    period: Period,
    now_ms: i64,
    tz_offset_minutes: i32,
    quota: HashMap<String, ProviderQuota>,
) -> UsageSummary {
    let bucket_ms = period.bucket_ms();
    let count = period.bucket_count();
    let offset_ms = tz_offset_minutes as i64 * 60_000;
    // Bucket alignment happens in LOCAL time, then shifts back to real
    // epoch ms. Aligning on raw epoch would split days at UTC midnight —
    // 02:00 for a CEST user — and label "Today" with UTC hours.
    //
    // The convention: `tz_offset_minutes` is minutes EAST of UTC, i.e.
    // `-new Date().getTimezoneOffset()` in JS. CEST (UTC+2) is `+120`;
    // New York in winter (UTC-5) is `-300`.
    let local_now = now_ms + offset_ms;
    let newest_local_start = local_now.div_euclid(bucket_ms) * bucket_ms;
    let local_start = newest_local_start - (count - 1) * bucket_ms;
    // Bounds are handed back in real epoch ms so row filtering below (and
    // the caller's SQL window) stays in the same clock as `created_at`.
    let start_ms = local_start - offset_ms;
    let end_ms = newest_local_start + bucket_ms - offset_ms;

    let mut buckets: Vec<UsageBucket> = (0..count)
        .map(|i| {
            let local = local_start + i * bucket_ms;
            let (label, sub_label) = bucket_labels(local, period);
            UsageBucket {
                // Epoch ms, like every other timestamp on the wire.
                start_ms: local - offset_ms,
                label,
                sub_label,
                providers: HashMap::new(),
            }
        })
        .collect();

    // Provider accumulators, keyed for stable output ordering later.
    let mut provider_acc: BTreeMap<String, ProviderAcc> = BTreeMap::new();
    let mut all_threads: HashSet<&str> = HashSet::new();
    let mut totals = UsageTotals::default();
    let mut cache_read_total: i64 = 0;
    let mut flat_acc: BTreeMap<(String, String), FlatAcc> = BTreeMap::new();
    let mut composition = UsageComposition::default();
    let mut provider_cost = 0.0_f64;
    let mut table_cost = 0.0_f64;
    let mut unpriced_tokens: i64 = 0;

    for row in rows {
        if row.created_at < start_ms || row.created_at >= end_ms {
            continue;
        }
        let tokens = row.total_tokens();
        let cost = row.cost_usd.unwrap_or(0.0);

        let index = ((row.created_at - start_ms).div_euclid(bucket_ms)) as usize;
        if let Some(bucket) = buckets.get_mut(index) {
            let slice = bucket.providers.entry(row.provider.clone()).or_default();
            slice.tokens += tokens;
            slice.cost_usd += cost;
        }

        let acc = provider_acc.entry(row.provider.clone()).or_default();
        acc.tokens += tokens;
        acc.input += row.input_tokens;
        acc.output += row.output_tokens;
        acc.cache_read += row.cache_read_tokens;
        acc.cache_write += row.cache_write_tokens;
        acc.cost += cost;
        acc.threads.insert(row.thread_id.clone());

        // An unattributed model is grouped under a single "unknown"
        // bucket rather than dropped: its tokens are real and must still
        // sum to the provider's total.
        let model_key_owned = row.model.clone().unwrap_or_else(|| "unknown".to_string());
        let model = acc.models.entry(model_key_owned.clone()).or_default();
        model.tokens += tokens;
        model.cost += cost;
        if row.subagent {
            model.subagent_tokens += tokens;
        }

        // Flat cross-provider model table + savings input.
        let flat = flat_acc
            .entry((row.provider.clone(), model_key_owned.clone()))
            .or_default();
        flat.tokens += tokens;
        flat.cost += cost;
        if row.cost_usd.is_some() {
            flat.priced = true;
        }
        if row.cost_source.as_deref() == Some("provider") {
            flat.provider_reported = true;
        } else if row.cost_source.as_deref() == Some("table") {
            flat.table_observed_input += row.observed_input();
            flat.table_output += row.output_tokens;
            flat.table_cost += cost;
        }

        // Composition strip.
        composition.processed_tokens += tokens;
        composition.input_tokens += row.input_tokens;
        composition.output_tokens += row.output_tokens;
        composition.cache_read_tokens += row.cache_read_tokens;
        composition.cache_write_tokens += row.cache_write_tokens;
        composition.reasoning_tokens += row.reasoning_tokens;

        // Cost confidence.
        match row.cost_source.as_deref() {
            Some("provider") => provider_cost += cost,
            Some("table") => table_cost += cost,
            _ => unpriced_tokens += tokens,
        }

        all_threads.insert(row.thread_id.as_str());
        totals.total_tokens += tokens;
        totals.estimated_cost_usd += cost;
        cache_read_total += row.cache_read_tokens;
    }

    totals.session_count = all_threads.len() as i64;
    totals.cache_read_share = if totals.total_tokens > 0 {
        cache_read_total as f64 / totals.total_tokens as f64
    } else {
        0.0
    };

    // Largest lane first — the dashboard reads top-down, and the design
    // puts the dominant provider at the top.
    let mut providers: Vec<ProviderUsage> = provider_acc
        .into_iter()
        .map(|(provider, acc)| acc.finish(provider))
        .collect();
    providers.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.provider.cmp(&b.provider)));

    // ── cache savings ──
    //
    // Per model, price what the SAME work would have cost with no cache
    // at all: every prompt token at the full input rate, output
    // unchanged. Only models the static table recognizes take part — a
    // catalogue-priced model has no raw-rate counterpart to compare
    // against, so including it would mean inventing one. The figure is
    // therefore an estimate over a subset, which the UI says out loud.
    let mut raw_total = 0.0_f64;
    let mut actual_for_savings = 0.0_f64;
    let mut savings = 0.0_f64;
    for ((_, model), acc) in &flat_acc {
        let Some(rates) = crate::agent_provider::pricing::lookup(model) else {
            continue;
        };
        let raw = (acc.table_observed_input as f64 * rates.input
            + acc.table_output as f64 * rates.output)
            / 1_000_000.0;
        raw_total += raw;
        actual_for_savings += acc.table_cost;
        // Clamp per model: a model whose actual cost exceeds the
        // no-cache hypothetical (possible with odd rate tables) must not
        // subtract from another model's genuine saving.
        savings += (raw - acc.table_cost).max(0.0);
    }
    composition.cache_savings_usd = savings;
    composition.cache_savings_multiplier = if actual_for_savings > 0.005 {
        Some(raw_total / actual_for_savings)
    } else {
        None
    };
    let observed_input = composition.input_tokens
        + composition.cache_read_tokens
        + composition.cache_write_tokens;
    composition.cache_read_share_of_input = if observed_input > 0 {
        composition.cache_read_tokens as f64 / observed_input as f64
    } else {
        0.0
    };

    let total_cost = provider_cost + table_cost;
    let confidence = CostConfidence {
        provider_reported_share: if total_cost > 0.0 {
            provider_cost / total_cost
        } else {
            0.0
        },
        table_priced_share: if total_cost > 0.0 {
            table_cost / total_cost
        } else {
            0.0
        },
        unpriced_token_share: if totals.total_tokens > 0 {
            unpriced_tokens as f64 / totals.total_tokens as f64
        } else {
            0.0
        },
        cache_savings_usd: savings,
    };

    // Priced models first by cost, then unpriced by tokens — an
    // unpriced row has no cost to sort by and would otherwise scatter
    // through the middle of the table.
    let mut models: Vec<FlatModelUsage> = flat_acc
        .into_iter()
        .map(|((provider, model), acc)| FlatModelUsage {
            provider,
            model,
            tokens: acc.tokens,
            cost_usd: acc.cost,
            priced: acc.priced,
            provider_reported: acc.provider_reported,
        })
        .collect();
    models.sort_by(|a, b| {
        b.priced
            .cmp(&a.priced)
            .then_with(|| {
                b.cost_usd
                    .partial_cmp(&a.cost_usd)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| b.tokens.cmp(&a.tokens))
            .then_with(|| a.model.cmp(&b.model))
    });

    UsageSummary {
        period: period.as_str().to_string(),
        start_ms,
        end_ms,
        buckets,
        providers,
        totals,
        composition,
        confidence,
        models,
        quota,
        synced_at_ms: now_ms,
    }
}

#[derive(Default)]
struct ProviderAcc {
    tokens: i64,
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    cost: f64,
    threads: HashSet<String>,
    models: HashMap<String, ModelAcc>,
}

#[derive(Default)]
struct ModelAcc {
    tokens: i64,
    cost: f64,
    subagent_tokens: i64,
}

/// Cross-provider per-model accumulator for the flat table and the
/// cache-savings estimate.
#[derive(Default)]
struct FlatAcc {
    tokens: i64,
    cost: f64,
    /// Savings only compare rows priced by the same static table.
    table_observed_input: i64,
    table_output: i64,
    table_cost: f64,
    priced: bool,
    provider_reported: bool,
}

impl ProviderAcc {
    fn finish(self, provider: String) -> ProviderUsage {
        let mut models: Vec<ModelUsage> = self
            .models
            .into_iter()
            .map(|(model, acc)| ModelUsage {
                model,
                tokens: acc.tokens,
                cost_usd: acc.cost,
                subagent_tokens: acc.subagent_tokens,
            })
            .collect();
        models.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.model.cmp(&b.model)));
        ProviderUsage {
            provider,
            tokens: self.tokens,
            input_tokens: self.input,
            output_tokens: self.output,
            cache_read_tokens: self.cache_read,
            cache_write_tokens: self.cache_write,
            cost_usd: self.cost,
            session_count: self.threads.len() as i64,
            models,
        }
    }
}

/// Axis and hover labels for a bucket.
///
/// Formatted here rather than in the frontend so the hourly and daily
/// cases share one definition of what a bucket *is*. Dates are derived
/// with plain arithmetic on the unix epoch (a civil-from-days
/// conversion) — the alternative is a date library dependency for two
/// label strings.
///
/// **`start_ms` is a LOCAL-shifted timestamp**, not epoch ms: the caller
/// has already added the timezone offset, so the hour and date read here
/// are the user's.
fn bucket_labels(start_ms: i64, period: Period) -> (String, String) {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    match period {
        Period::Today => {
            let hour = (start_ms.rem_euclid(DAY_MS)) / HOUR_MS;
            (format!("{hour}:00"), format!("Today {hour}:00"))
        }
        _ => {
            let days = start_ms.div_euclid(DAY_MS);
            let (_, month, day) = civil_from_days(days);
            (
                day.to_string(),
                format!("{} {}", MONTHS[(month - 1) as usize], day),
            )
        }
    }
}

/// Days since the unix epoch → `(year, month, day)`.
///
/// Howard Hinnant's `civil_from_days`, the standard branch-free
/// algorithm behind `std::chrono`. Inlined because pulling a date crate
/// in to render "Aug 8" would be the larger dependency.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Aggregate usage for `period` (`"today"` | `"7d"` | `"30d"`).
#[tauri::command]
pub async fn usage_summary(
    period: String,
    tz_offset_minutes: i32,
    db: State<'_, DatabaseStore>,
    quota: State<'_, PlanQuotaStore>,
) -> Result<UsageSummary, String> {
    let period = Period::parse(&period)?;
    let tz_offset_minutes = sanitize_tz_offset(tz_offset_minutes);
    let now = now_ms();
    // Over-fetch by two buckets and let `summarize` do the exact
    // windowing — the query only needs to be a cheap index range scan,
    // not the authority on where the window starts. Two rather than one
    // because the local-time alignment can shift the window by up to a
    // full bucket in either direction.
    let since = now - (period.bucket_count() + 2) * period.bucket_ms();
    let rows = db.usage_rows_since(since)?;
    Ok(summarize(&rows, period, now, tz_offset_minutes, quota.snapshot()))
}

/// Clamp a client-supplied timezone offset to the range real zones use.
///
/// Real offsets span UTC-12:00 to UTC+14:00. Anything outside that is a
/// bad caller, and letting it through would move the query window by an
/// arbitrary amount; clamping degrades to UTC-ish behavior instead.
fn sanitize_tz_offset(minutes: i32) -> i32 {
    minutes.clamp(-12 * 60, 14 * 60)
}

/// The same data as a CSV the frontend downloads as a blob.
///
/// One row per bucket × provider × model, which is the finest grain that
/// is still meaningful to a human opening it in a spreadsheet (raw
/// ledger rows would be per-assistant-message and unreadable).
#[tauri::command]
pub async fn usage_export_csv(
    period: String,
    tz_offset_minutes: i32,
    db: State<'_, DatabaseStore>,
) -> Result<String, String> {
    let period = Period::parse(&period)?;
    let tz_offset_minutes = sanitize_tz_offset(tz_offset_minutes);
    let now = now_ms();
    let since = now - (period.bucket_count() + 2) * period.bucket_ms();
    let rows = db.usage_rows_since(since)?;
    // The CSV is a history export; live quota levels have no place in it.
    Ok(to_csv(&rows, period, now, tz_offset_minutes))
}

fn to_csv(
    rows: &[UsageLedgerRow],
    period: Period,
    now: i64,
    tz_offset_minutes: i32,
) -> String {
    let bucket_ms = period.bucket_ms();
    let offset_ms = tz_offset_minutes as i64 * 60_000;
    let summary = summarize(rows, period, now, tz_offset_minutes, HashMap::new());

    // Re-group the raw rows on the same bucket boundaries `summarize`
    // used, so the CSV and the chart cannot disagree.
    let mut grouped: BTreeMap<(i64, String, String), [i64; 6]> = BTreeMap::new();
    let mut costs: BTreeMap<(i64, String, String), f64> = BTreeMap::new();
    // Provenance is per row; a bucket mixing sources reports "mixed"
    // rather than silently picking one.
    let mut sources: BTreeMap<(i64, String, String), Option<String>> = BTreeMap::new();
    for row in rows {
        if row.created_at < summary.start_ms || row.created_at >= summary.end_ms {
            continue;
        }
        // Group on the same LOCAL boundaries the chart uses, so the CSV
        // and the chart cannot disagree about which day a row is in.
        let bucket = (row.created_at + offset_ms).div_euclid(bucket_ms) * bucket_ms;
        let model = row.model.clone().unwrap_or_else(|| "unknown".to_string());
        let key = (bucket, row.provider.clone(), model);
        let entry = grouped.entry(key.clone()).or_default();
        entry[0] += row.input_tokens;
        entry[1] += row.output_tokens;
        entry[2] += row.cache_read_tokens;
        entry[3] += row.cache_write_tokens;
        entry[4] += row.total_tokens();
        entry[5] += row.reasoning_tokens;
        *costs.entry(key.clone()).or_default() += row.cost_usd.unwrap_or(0.0);
        let source = sources.entry(key).or_insert_with(|| row.cost_source.clone());
        if source.as_deref() != row.cost_source.as_deref() {
            *source = Some("mixed".to_string());
        }
    }

    let mut out = String::from(
        "bucket_start,provider,model,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,reasoning_tokens,total_tokens,cost_usd,cost_source\n",
    );
    for ((bucket, provider, model), counts) in &grouped {
        let (_, sub_label) = bucket_labels(*bucket, period);
        let cost = costs.get(&(*bucket, provider.clone(), model.clone())).copied().unwrap_or(0.0);
        let source = sources
            .get(&(*bucket, provider.clone(), model.clone()))
            .and_then(|s| s.clone())
            .unwrap_or_default();
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{:.4},{}\n",
            csv_field(&sub_label),
            csv_field(provider),
            csv_field(model),
            counts[0],
            counts[1],
            counts[2],
            counts[3],
            counts[5],
            counts[4],
            cost,
            csv_field(&source),
        ));
    }
    out
}

/// Quote a CSV field when it contains a separator, quote, or newline.
/// Model ids carry slashes and dots, never commas today — but they come
/// from upstream catalogues, so the escape is not optional.
fn csv_field(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_provider::PlanWindowKind;

    fn row(created_at: i64, provider: &str, model: &str, tokens: [i64; 4], cost: f64) -> UsageLedgerRow {
        UsageLedgerRow {
            created_at,
            thread_id: "t1".into(),
            workspace_id: Some("ws1".into()),
            provider: provider.into(),
            model: Some(model.into()),
            subagent: false,
            input_tokens: tokens[0],
            output_tokens: tokens[1],
            cache_read_tokens: tokens[2],
            cache_write_tokens: tokens[3],
            reasoning_tokens: 0,
            cost_usd: Some(cost),
            // Every fixture row is table-priced unless a test says
            // otherwise; matches what Claude/Codex actually emit.
            cost_source: Some("table".into()),
            source: "live".into(),
        }
    }

    /// A fixed "now" pinned to an exact UTC midnight, so local-offset
    /// arithmetic in the timezone tests below reads directly (UTC hour 0
    /// → hour 2 at UTC+2, hour 19 the previous day at UTC-5).
    const NOW: i64 = 1_799_971_200_000;

    #[test]
    fn buckets_are_hourly_for_today_and_daily_otherwise() {
        assert_eq!(summarize(&[], Period::Today, NOW, 0, HashMap::new()).buckets.len(), 24);
        assert_eq!(summarize(&[], Period::Days7, NOW, 0, HashMap::new()).buckets.len(), 7);
        assert_eq!(summarize(&[], Period::Days30, NOW, 0, HashMap::new()).buckets.len(), 30);
    }

    #[test]
    fn rows_land_in_the_bucket_containing_them() {
        let s = summarize(
            &[
                row(NOW, "claude", "claude-opus-4-5", [10, 5, 0, 0], 1.0),
                // Two days back.
                row(NOW - 2 * DAY_MS, "claude", "claude-opus-4-5", [7, 3, 0, 0], 0.5),
            ],
            Period::Days7,
            NOW,
            0,
            HashMap::new(),
        );
        let last = s.buckets.last().unwrap();
        assert_eq!(last.providers["claude"].tokens, 15);
        assert_eq!(s.buckets[4].providers["claude"].tokens, 10);
        assert_eq!(s.totals.total_tokens, 25);
    }

    #[test]
    fn rows_outside_the_window_are_ignored() {
        let s = summarize(
            &[row(NOW - 40 * DAY_MS, "claude", "m", [100, 100, 0, 0], 9.0)],
            Period::Days7,
            NOW,
            0,
            HashMap::new(),
        );
        assert_eq!(s.totals.total_tokens, 0);
        assert_eq!(s.providers.len(), 0);
    }

    #[test]
    fn estimated_cost_sums_every_provider_without_guessing_billing() {
        let s = summarize(
            &[
                row(NOW, "claude", "claude-opus-4-5", [100, 0, 0, 0], 2.0),
                row(NOW, "codex", "gpt-5-codex", [100, 0, 0, 0], 3.0),
                row(NOW, "opencode", "openrouter/kimi", [100, 0, 0, 0], 4.0),
            ],
            Period::Today,
            NOW,
            0,
            HashMap::new(),
        );
        assert_eq!(s.totals.estimated_cost_usd, 9.0);
    }

    #[test]
    fn subagent_tokens_are_counted_and_attributed() {
        let mut sub = row(NOW, "claude", "claude-haiku-4-5", [40, 10, 0, 0], 0.1);
        sub.subagent = true;
        let s = summarize(
            &[row(NOW, "claude", "claude-opus-4-5", [100, 0, 0, 0], 1.0), sub],
            Period::Today,
            NOW,
            0,
            HashMap::new(),
        );
        let claude = &s.providers[0];
        assert_eq!(claude.tokens, 150);
        let haiku = claude.models.iter().find(|m| m.model.contains("haiku")).unwrap();
        assert_eq!(haiku.tokens, 50);
        assert_eq!(haiku.subagent_tokens, 50);
        let opus = claude.models.iter().find(|m| m.model.contains("opus")).unwrap();
        assert_eq!(opus.subagent_tokens, 0);
    }

    #[test]
    fn distinct_provider_sessions_are_counted_once() {
        let mut a = row(NOW, "claude", "m", [10, 0, 0, 0], 0.0);
        a.thread_id = "t1".into();
        let mut b = row(NOW, "claude", "m", [10, 0, 0, 0], 0.0);
        b.thread_id = "t1".into(); // same thread, second increment
        let mut c = row(NOW, "codex", "m", [10, 0, 0, 0], 0.0);
        c.thread_id = "t2".into();
        let s = summarize(&[a, b, c], Period::Today, NOW, 0, HashMap::new());
        assert_eq!(s.totals.session_count, 2);
        assert_eq!(s.providers.iter().find(|p| p.provider == "claude").unwrap().session_count, 1);
    }

    #[test]
    fn cache_read_share_is_a_fraction_of_all_tokens() {
        let s = summarize(
            &[row(NOW, "claude", "m", [25, 25, 50, 0], 0.0)],
            Period::Today,
            NOW,
            0,
            HashMap::new(),
        );
        assert_eq!(s.totals.total_tokens, 100);
        assert!((s.totals.cache_read_share - 0.5).abs() < 1e-9);
        // No rows must not divide by zero.
        assert_eq!(summarize(&[], Period::Today, NOW, 0, HashMap::new()).totals.cache_read_share, 0.0);
    }

    #[test]
    fn rows_with_no_model_are_grouped_not_dropped() {
        let mut r = row(NOW, "opencode", "x", [10, 10, 0, 0], 0.0);
        r.model = None;
        let s = summarize(&[r], Period::Today, NOW, 0, HashMap::new());
        assert_eq!(s.providers[0].tokens, 20);
        assert_eq!(s.providers[0].models[0].model, "unknown");
        assert_eq!(s.providers[0].models[0].tokens, 20);
    }

    #[test]
    fn provider_token_split_sums_to_the_total() {
        let s = summarize(
            &[row(NOW, "claude", "m", [1, 2, 4, 8], 0.0)],
            Period::Today,
            NOW,
            0,
            HashMap::new(),
        );
        let p = &s.providers[0];
        assert_eq!(
            p.input_tokens + p.output_tokens + p.cache_read_tokens + p.cache_write_tokens,
            p.tokens
        );
        assert_eq!(p.tokens, 15);
    }

    // ── timezone ──

    /// `NOW` is a UTC day boundary, so a positive offset must push the
    /// newest daily bucket into the NEXT local day.
    #[test]
    fn daily_buckets_split_at_local_midnight_east_of_utc() {
        // UTC+2 (CEST). A row at 00:30 UTC is 02:30 local on the SAME day;
        // a row at 23:00 UTC is 01:00 local on the NEXT day.
        let late = NOW + 23 * HOUR_MS;
        let s = summarize(
            &[
                row(NOW + 30 * 60 * 1000, "claude", "m", [10, 0, 0, 0], 0.0),
                row(late, "claude", "m", [7, 0, 0, 0], 0.0),
            ],
            Period::Days7,
            late,
            120,
            HashMap::new(),
        );
        // The two rows must land in different buckets — under UTC
        // bucketing they would share one.
        let occupied: Vec<usize> = s
            .buckets
            .iter()
            .enumerate()
            .filter(|(_, b)| !b.providers.is_empty())
            .map(|(i, _)| i)
            .collect();
        assert_eq!(occupied.len(), 2, "local midnight split them");
        assert_eq!(s.totals.total_tokens, 17);
    }

    /// And a negative offset pulls the boundary the other way.
    #[test]
    fn daily_buckets_split_at_local_midnight_west_of_utc() {
        // UTC-5 (New York, winter). 00:30 UTC is 19:30 local the PREVIOUS
        // day; 06:00 UTC is 01:00 local the same day.
        let later = NOW + 6 * HOUR_MS;
        let s = summarize(
            &[
                row(NOW + 30 * 60 * 1000, "claude", "m", [10, 0, 0, 0], 0.0),
                row(later, "claude", "m", [7, 0, 0, 0], 0.0),
            ],
            Period::Days7,
            later,
            -300,
            HashMap::new(),
        );
        let occupied: Vec<usize> = s
            .buckets
            .iter()
            .enumerate()
            .filter(|(_, b)| !b.providers.is_empty())
            .map(|(i, _)| i)
            .collect();
        assert_eq!(occupied.len(), 2, "local midnight split them");
        assert_eq!(s.totals.total_tokens, 17);
    }

    /// Hourly labels must read as the user's local clock, not UTC.
    #[test]
    fn hour_labels_follow_the_local_clock() {
        // NOW is a UTC day boundary, so UTC hour 0.
        let utc = summarize(&[], Period::Today, NOW, 0, HashMap::new());
        assert_eq!(utc.buckets.last().unwrap().label, "0:00");

        // UTC+2 → the same instant is 02:00 local.
        let east = summarize(&[], Period::Today, NOW, 120, HashMap::new());
        assert_eq!(east.buckets.last().unwrap().label, "2:00");
        assert_eq!(east.buckets.last().unwrap().sub_label, "Today 2:00");

        // UTC-5 → 19:00 local, on the previous day.
        let west = summarize(&[], Period::Today, NOW, -300, HashMap::new());
        assert_eq!(west.buckets.last().unwrap().label, "19:00");
    }

    /// The window bounds stay in real epoch ms, so row filtering and the
    /// caller's SQL range keep the same clock as `created_at`.
    #[test]
    fn window_bounds_remain_epoch_ms() {
        let s = summarize(&[], Period::Today, NOW, 120, HashMap::new());
        // 24 hourly buckets, and the newest one contains `NOW`.
        assert_eq!(s.end_ms - s.start_ms, 24 * HOUR_MS);
        assert!(s.start_ms <= NOW && NOW < s.end_ms);
        // Bucket starts are epoch ms too, spaced one hour apart.
        assert_eq!(s.buckets[1].start_ms - s.buckets[0].start_ms, HOUR_MS);
        assert_eq!(s.buckets[0].start_ms, s.start_ms);
    }

    /// A nonsense offset must not move the query window arbitrarily.
    #[test]
    fn tz_offset_is_clamped_to_real_zones() {
        assert_eq!(sanitize_tz_offset(0), 0);
        assert_eq!(sanitize_tz_offset(120), 120);
        assert_eq!(sanitize_tz_offset(-300), -300);
        assert_eq!(sanitize_tz_offset(14 * 60), 14 * 60);
        assert_eq!(sanitize_tz_offset(99_999), 14 * 60);
        assert_eq!(sanitize_tz_offset(-99_999), -12 * 60);
    }

    /// The CSV groups on the same local boundaries as the chart.
    #[test]
    fn csv_groups_on_local_boundaries_too() {
        let late = NOW + 23 * HOUR_MS;
        let csv = to_csv(
            &[
                row(NOW + 30 * 60 * 1000, "claude", "m", [10, 0, 0, 0], 0.0),
                row(late, "claude", "m", [7, 0, 0, 0], 0.0),
            ],
            Period::Days7,
            late,
            120,
        );
        // Two distinct local days → two rows, not one merged row.
        assert_eq!(csv.trim().lines().count(), 3, "header + two day rows");
    }

    // ── plan quota store + detected billing ──

    fn window(kind: PlanWindowKind, pct: f64) -> PlanUsageWindow {
        PlanUsageWindow {
            kind,
            used_pct: pct,
            resets_at_ms: None,
            label: None,
        }
    }

    /// The two halves of Codex's story arrive in separate events —
    /// plan/auth from `account/read`, windows from `rateLimits/*`. A
    /// blind overwrite would let each erase the other.
    #[test]
    fn store_merges_partial_snapshots_instead_of_clobbering() {
        let store = PlanQuotaStore::default();
        // account/read: auth + label, no windows.
        store.record("codex", vec![], Some("ChatGPT Pro".into()),
                     Some(PlanAuthMode::Subscription), 100);
        // rateLimits: windows, no auth.
        store.record("codex", vec![window(PlanWindowKind::FiveHour, 41.0)], None, None, 200);

        let snap = store.snapshot();
        let codex = &snap["codex"];
        assert_eq!(codex.windows.len(), 1, "windows landed");
        assert_eq!(codex.plan_label.as_deref(), Some("ChatGPT Pro"), "label survived");
        assert_eq!(codex.auth_mode, Some(PlanAuthMode::Subscription), "auth survived");
        assert_eq!(codex.received_at_ms, 200);
    }

    /// Within a field the newest wins — a snapshot is a level, not a sum.
    #[test]
    fn store_replaces_windows_wholesale_on_a_fresh_reading() {
        let store = PlanQuotaStore::default();
        store.record("claude", vec![window(PlanWindowKind::FiveHour, 10.0)], None, None, 1);
        store.record("claude", vec![window(PlanWindowKind::FiveHour, 55.0)], None, None, 2);
        let snap = store.snapshot();
        assert_eq!(snap["claude"].windows.len(), 1);
        assert_eq!(snap["claude"].windows[0].used_pct, 55.0);
    }

    #[test]
    fn store_reports_nothing_for_an_unseen_provider() {
        let store = PlanQuotaStore::default();
        assert!(store.snapshot().is_empty());
        assert!(store.auth_mode_for("claude").is_none());
    }

    /// Cost estimation and subscription quota are deliberately independent.
    #[test]
    fn summary_reports_api_equivalent_cost_alongside_quota() {
        let mut quota = HashMap::new();
        quota.insert(
            "claude".to_string(),
            ProviderQuota {
                windows: vec![window(PlanWindowKind::FiveHour, 41.0)],
                plan_label: Some("Max 20x".into()),
                auth_mode: Some(PlanAuthMode::ApiKey),
                received_at_ms: 1,
            },
        );
        let s = summarize(
            &[row(NOW, "claude", "claude-opus-4-5", [100, 0, 0, 0], 3.0)],
            Period::Today,
            NOW,
            0,
            quota,
        );
        assert_eq!(s.totals.estimated_cost_usd, 3.0);
        assert_eq!(s.quota["claude"].windows.len(), 1);
        assert_eq!(s.quota["claude"].plan_label.as_deref(), Some("Max 20x"));
    }

    // ── composition strip ──

    #[test]
    fn composition_splits_the_period_by_where_tokens_went() {
        let s = summarize(
            &[row(NOW, "claude", "claude-opus-4-5", [100, 50, 800, 40], 1.0)],
            Period::Today,
            NOW,
            0,
            HashMap::new(),
        );
        let c = &s.composition;
        assert_eq!(c.processed_tokens, 990, "the four-way sum");
        assert_eq!(c.input_tokens, 100);
        assert_eq!(c.output_tokens, 50);
        assert_eq!(c.cache_read_tokens, 800);
        assert_eq!(c.cache_write_tokens, 40);
        // Denominator is OBSERVED INPUT (100 + 800 + 40), not all tokens
        // — a cache hit replaces input, never output.
        assert!((c.cache_read_share_of_input - 800.0 / 940.0).abs() < 1e-9);
    }

    #[test]
    fn composition_reports_reasoning_only_when_a_provider_split_it() {
        // Claude never reports a split.
        let claude = summarize(
            &[row(NOW, "claude", "claude-opus-4-5", [10, 10, 0, 0], 0.0)],
            Period::Today,
            NOW,
            0,
            HashMap::new(),
        );
        assert_eq!(claude.composition.reasoning_tokens, 0);

        let mut r = row(NOW, "codex", "gpt-5-codex", [10, 40, 0, 0], 0.0);
        r.reasoning_tokens = 25;
        let codex = summarize(&[r], Period::Today, NOW, 0, HashMap::new());
        assert_eq!(codex.composition.reasoning_tokens, 25);
        // …and it stays INSIDE output, never added to the total.
        assert_eq!(codex.composition.processed_tokens, 50);
        assert_eq!(codex.composition.output_tokens, 40);
    }

    /// Savings price the same work with no cache at all: every prompt
    /// token at full input rate.
    #[test]
    fn cache_savings_compare_against_the_uncached_list_price() {
        // sonnet: $3/Mtok input, $15/Mtok output.
        // 1M observed input (of which 900k was cache reads) + 100k output.
        let mut r = row(NOW, "claude", "claude-sonnet-4-5", [100_000, 100_000, 900_000, 0], 0.0);
        // Actual cost: 100k fresh input @3 + 100k output @15 + 900k reads @0.3
        r.cost_usd = Some(0.3 + 1.5 + 0.27);
        let s = summarize(&[r], Period::Today, NOW, 0, HashMap::new());
        // Raw = 1M input @3 + 100k output @15 = 3.0 + 1.5 = 4.5
        let actual = 0.3 + 1.5 + 0.27;
        assert!((s.composition.cache_savings_usd - (4.5 - actual)).abs() < 1e-6);
        let mult = s.composition.cache_savings_multiplier.unwrap();
        assert!((mult - 4.5 / actual).abs() < 1e-6);
    }

    /// Never negative, per model.
    #[test]
    fn cache_savings_clamp_at_zero_per_model() {
        let mut r = row(NOW, "claude", "claude-sonnet-4-5", [10, 10, 0, 0], 0.0);
        r.cost_usd = Some(500.0); // absurdly above any raw price
        let s = summarize(&[r], Period::Today, NOW, 0, HashMap::new());
        assert_eq!(s.composition.cache_savings_usd, 0.0);
    }

    #[test]
    fn cache_savings_omit_the_multiplier_when_nothing_was_spent() {
        let mut r = row(NOW, "claude", "claude-sonnet-4-5", [10, 10, 0, 0], 0.0);
        r.cost_usd = Some(0.0);
        let s = summarize(&[r], Period::Today, NOW, 0, HashMap::new());
        assert!(s.composition.cache_savings_multiplier.is_none());
    }

    /// A model the static table cannot price has no raw-rate
    /// counterpart, so it sits out of the savings estimate entirely.
    #[test]
    fn cache_savings_exclude_models_with_no_table_rates() {
        let mut r = row(NOW, "opencode", "openrouter/kimi-k2", [1_000_000, 0, 0, 0], 2.0);
        r.cost_source = Some("provider".into());
        let s = summarize(&[r], Period::Today, NOW, 0, HashMap::new());
        assert_eq!(s.composition.cache_savings_usd, 0.0);
        assert!(s.composition.cache_savings_multiplier.is_none());
    }

    // ── cost confidence ──

    #[test]
    fn confidence_splits_cost_by_source_and_tokens_by_unpriced() {
        let mut provider_row = row(NOW, "opencode", "anthropic/x", [100, 0, 0, 0], 3.0);
        provider_row.cost_source = Some("provider".into());
        let table_row = row(NOW, "claude", "claude-opus-4-5", [100, 0, 0, 0], 1.0);
        let mut unpriced = row(NOW, "opencode", "openrouter/kimi-k2", [800, 0, 0, 0], 0.0);
        unpriced.cost_usd = None;
        unpriced.cost_source = None;

        let s = summarize(
            &[provider_row, table_row, unpriced],
            Period::Today,
            NOW,
            0,
            HashMap::new(),
        );
        let c = &s.confidence;
        assert!((c.provider_reported_share - 0.75).abs() < 1e-9, "3 of $4");
        assert!((c.table_priced_share - 0.25).abs() < 1e-9);
        // Unpriced is a TOKEN share — a cost share would always be 0 and
        // hide the very thing this row exists to show.
        assert!((c.unpriced_token_share - 800.0 / 1000.0).abs() < 1e-9);
    }

    #[test]
    fn confidence_is_all_zero_rather_than_nan_on_an_empty_period() {
        let c = summarize(&[], Period::Today, NOW, 0, HashMap::new()).confidence;
        assert_eq!(c.provider_reported_share, 0.0);
        assert_eq!(c.table_priced_share, 0.0);
        assert_eq!(c.unpriced_token_share, 0.0);
    }

    // ── flat model table ──

    #[test]
    fn flat_models_span_providers_and_sort_priced_first_by_cost() {
        let mut unpriced = row(NOW, "opencode", "openrouter/kimi-k2", [9_000, 0, 0, 0], 0.0);
        unpriced.cost_usd = None;
        unpriced.cost_source = None;
        let s = summarize(
            &[
                row(NOW, "claude", "claude-opus-4-5", [100, 0, 0, 0], 5.0),
                row(NOW, "codex", "gpt-5-codex", [100, 0, 0, 0], 9.0),
                unpriced,
            ],
            Period::Today,
            NOW,
            0,
            HashMap::new(),
        );
        let names: Vec<&str> = s.models.iter().map(|m| m.model.as_str()).collect();
        // Priced by cost desc, then the unpriced row last.
        assert_eq!(names, vec!["gpt-5-codex", "claude-opus-4-5", "openrouter/kimi-k2"]);
        assert_eq!(s.models[0].provider, "codex");
        assert!(s.models[0].priced);
        assert!(!s.models[2].priced, "unpriced row is flagged for the em-dash");
    }

    #[test]
    fn flat_models_flag_provider_reported_pricing() {
        let mut r = row(NOW, "opencode", "anthropic/claude-sonnet-4-5", [10, 0, 0, 0], 1.0);
        r.cost_source = Some("provider".into());
        let s = summarize(&[r], Period::Today, NOW, 0, HashMap::new());
        assert!(s.models[0].provider_reported);
    }

    // ── 90 days ──

    #[test]
    fn ninety_day_period_has_ninety_daily_buckets() {
        assert_eq!(Period::parse("90d").unwrap(), Period::Days90);
        let s = summarize(&[], Period::Days90, NOW, 0, HashMap::new());
        assert_eq!(s.buckets.len(), 90);
        assert_eq!(s.period, "90d");
        // Daily, like 7d/30d — and the window spans 90 days.
        assert_eq!(s.end_ms - s.start_ms, 90 * DAY_MS);
    }

    #[test]
    fn ninety_day_buckets_respect_the_local_offset() {
        let s = summarize(&[], Period::Days90, NOW, 120, HashMap::new());
        assert_eq!(s.buckets.len(), 90);
        assert_eq!(s.buckets[1].start_ms - s.buckets[0].start_ms, DAY_MS);
        assert!(s.start_ms <= NOW && NOW < s.end_ms);
    }

    #[test]
    fn csv_carries_reasoning_and_cost_source() {
        let mut r = row(NOW, "codex", "gpt-5-codex", [10, 40, 0, 0], 1.0);
        r.reasoning_tokens = 25;
        let csv = to_csv(&[r], Period::Days7, NOW, 0);
        let lines: Vec<&str> = csv.trim().lines().collect();
        assert!(lines[0].contains("reasoning_tokens"));
        assert!(lines[0].ends_with("cost_source"));
        assert!(lines[1].contains(",25,"), "reasoning column");
        assert!(lines[1].ends_with("table"));
    }

    // ── provider-history inclusion ──

    /// Every provider-history row counts independent of which app launched it.
    #[test]
    fn imported_rows_always_count() {
        let mut imported = row(NOW, "claude", "claude-opus-4-5", [100, 0, 0, 0], 2.0);
        imported.source = "provider_history".into();
        imported.thread_id = "claude:sess-9".into();
        let live = row(NOW, "claude", "claude-opus-4-5", [10, 0, 0, 0], 0.5);

        let s = summarize(&[imported, live], Period::Today, NOW, 0, HashMap::new());
        assert_eq!(s.totals.total_tokens, 110);
        assert_eq!(s.totals.session_count, 2);
    }

    #[test]
    fn imported_rows_blend_into_the_existing_provider_lane() {
        let mut imported = row(NOW, "claude", "claude-opus-4-5", [100, 0, 0, 0], 2.0);
        imported.source = "provider_history".into();
        let s = summarize(
            &[imported, row(NOW, "claude", "claude-opus-4-5", [10, 0, 0, 0], 0.5)],
            Period::Today,
            NOW,
            0,
            HashMap::new(),
        );
        assert_eq!(s.providers.len(), 1, "one Claude lane, not two");
        assert_eq!(s.providers[0].tokens, 110);
    }

    #[test]
    fn csv_includes_imported_rows_too() {
        let mut imported = row(NOW, "claude", "cli-model", [100, 0, 0, 0], 2.0);
        imported.source = "provider_history".into();
        let rows = [imported, row(NOW, "claude", "live-model", [10, 0, 0, 0], 0.5)];
        let csv = to_csv(&rows, Period::Days7, NOW, 0);
        assert!(csv.contains("cli-model"));
        assert!(csv.contains("live-model"));
    }

    #[test]
    fn period_parsing_rejects_unknown_values() {
        assert!(Period::parse("today").is_ok());
        assert!(Period::parse("7d").is_ok());
        assert!(Period::parse("30d").is_ok());
        assert!(Period::parse("all").is_err());
    }

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        // 2024 was a leap year — the day after Feb 28 is Feb 29.
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    }

    #[test]
    fn csv_has_a_header_and_one_row_per_bucket_provider_model() {
        let csv = to_csv(
            &[
                row(NOW, "claude", "claude-opus-4-5", [10, 5, 0, 0], 1.5),
                row(NOW, "claude", "claude-opus-4-5", [1, 1, 0, 0], 0.5),
                row(NOW, "opencode", "openrouter/kimi", [3, 3, 0, 0], 0.25),
            ],
            Period::Days7,
            NOW,
            0,
        );
        let lines: Vec<&str> = csv.trim().lines().collect();
        assert!(lines[0].starts_with("bucket_start,provider,model,"));
        // Two claude rows collapse into one; opencode is its own row.
        assert_eq!(lines.len(), 3);
        assert!(lines.iter().any(|l| l.contains("claude-opus-4-5") && l.contains(",17,")));
        assert!(lines.iter().any(|l| l.contains("2.0000")));
    }

    #[test]
    fn csv_quotes_fields_containing_separators() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    }
}
