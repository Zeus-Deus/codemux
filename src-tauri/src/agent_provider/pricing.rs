//! Static list-price table for the usage ledger.
//!
//! The Settings → Usage dashboard reports what a turn *would* cost at
//! published list prices, regardless of how the user actually pays for it
//! (a Claude Max subscription, a ChatGPT plan, or metered API keys). It is an
//! API-equivalent estimate, not a claim about what a plan covered or what the
//! provider billed.
//!
//! Only verified model ids and their dated snapshots are matched. Provider
//! prefixes and Claude context suffixes are normalized first. Future versions
//! and internal routing labels remain unpriced rather than inheriting a
//! different model's price. Their tokens still appear in the dashboard.
//!
//! These are current standard, global, short-context token rates. Histories
//! do not consistently preserve per-request context size, service tier,
//! geography, or historical price schedules. This fallback cannot reproduce
//! those billing modifiers or server-side tool fees. See the usage audit in
//! `docs/research/usage-tracker-audit.md` for the provider/source boundaries.
//!
//! OpenCode is the exception: its durable message records can carry a cost
//! calculated from the upstream model catalogue, so the history importer
//! prefers that and only falls back to this table when it is absent.

/// List price for one model family, in **USD per million tokens**.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelRates {
    /// Uncached input tokens.
    pub input: f64,
    /// Output tokens (reasoning included — providers bill it as output).
    pub output: f64,
    /// Tokens served from a cache hit.
    pub cache_read: f64,
    /// Tokens written into the cache on a miss, at the **5-minute** TTL.
    pub cache_write: f64,
    /// Tokens written into the cache at the **1-hour** TTL.
    ///
    /// Anthropic charges the longer TTL at 2x base input against 1.25x
    /// for the 5-minute tier, and Claude Code uses both. Measured across
    /// all 1,334 transcripts on this machine, 70.9M of 176.6M
    /// cache-creation tokens (40.2%) were `ephemeral_1h_input_tokens` —
    /// but the share swings from 18.7% to 100% by day and 24.6% to 63.5%
    /// by model, so it is a real distinction rather than a constant that
    /// could be folded into one blended rate. Pricing every write at
    /// 1.25x understated that corpus by roughly $419 at list.
    ///
    /// Providers that publish no 1-hour tier repeat their `cache_write`
    /// rate here, so the split is a no-op for them.
    pub cache_write_1h: f64,
}

impl ModelRates {
    /// Anthropic's standard cache reads cost 0.1x input. Newer models
    /// override that rate; write multipliers remain 1.25x and 2x.
    const fn anthropic(input: f64, output: f64) -> Self {
        Self::anthropic_with_cache_read(input, output, input * 0.1)
    }

    const fn anthropic_with_cache_read(input: f64, output: f64, cache_read: f64) -> Self {
        Self {
            input,
            output,
            cache_read,
            cache_write: input * 1.25,
            cache_write_1h: input * 2.0,
        }
    }

    /// OpenAI-style pricing, where the cached-input rate is published
    /// directly rather than derived, and no separate charge for writing
    /// to the cache is published for this model.
    ///
    /// Models without a published separate cache-write charge use zero.
    const fn openai(input: f64, output: f64, cache_read: f64) -> Self {
        Self {
            input,
            output,
            cache_read,
            cache_write: 0.0,
            cache_write_1h: 0.0,
        }
    }

    /// GPT-5.6 and GPT-6 publish a 1.25x cache-write rate. The cache-read
    /// rate is explicit: GPT-6.1 Sol reads at 0.05x instead of 0.1x.
    const fn openai_with_cache_write(input: f64, output: f64, cache_read: f64) -> Self {
        Self {
            input,
            output,
            cache_read,
            cache_write: input * 1.25,
            cache_write_1h: input * 1.25,
        }
    }

    /// Cost in USD for one non-overlapping token split.
    ///
    /// The four counts must be disjoint — that invariant is established by
    /// each history importer before materialization, so this function can be
    /// a plain dot product.
    pub fn cost_usd(&self, input: u64, output: u64, cache_read: u64, cache_write: u64) -> f64 {
        self.cost_usd_with_1h(input, output, cache_read, cache_write, 0)
    }

    /// As [`cost_usd`](Self::cost_usd), but splitting the cache writes
    /// across the two TTL tiers.
    ///
    /// `cache_write_1h` is a **subset** of `cache_write`, not a sibling —
    /// the caller reports the total it will store in the ledger plus how
    /// much of it was the longer TTL, and the remainder bills at the
    /// 5-minute rate. Passing a subset rather than two disjoint counts
    /// means a caller that cannot see the split still gets the old
    /// behavior by passing `0`, and a caller that over-reports the 1-hour
    /// share cannot inflate the total (it is clamped).
    pub fn cost_usd_with_1h(
        &self,
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write: u64,
        cache_write_1h: u64,
    ) -> f64 {
        const PER_MILLION: f64 = 1_000_000.0;
        let one_hour = cache_write_1h.min(cache_write);
        let five_minute = cache_write - one_hour;
        (input as f64 * self.input
            + output as f64 * self.output
            + cache_read as f64 * self.cache_read
            + five_minute as f64 * self.cache_write
            + one_hour as f64 * self.cache_write_1h)
            / PER_MILLION
    }
}

/// Verified standard/global token rates, rechecked 2026-10-01.
///
/// Sources:
/// - <https://platform.claude.com/docs/en/about-claude/pricing>
/// - <https://developers.openai.com/api/docs/pricing>
///
/// Keep generations explicit. A new version can change both base and cache
/// prices; family catch-alls silently misprice it. A price correction also
/// requires an IMPORT_VERSION bump so unchanged histories are re-priced.
const RATES: &[(&str, ModelRates)] = &[
    // Anthropic. Opus 5.5 and Fable/Mythos 5.1 have lower cache-read ratios.
    (
        "claude-opus-5-5",
        ModelRates::anthropic_with_cache_read(4.0, 20.0, 0.20),
    ),
    ("claude-opus-5", ModelRates::anthropic(5.0, 25.0)),
    ("claude-opus-4-8", ModelRates::anthropic(5.0, 25.0)),
    ("claude-opus-4-7", ModelRates::anthropic(5.0, 25.0)),
    ("claude-opus-4-6", ModelRates::anthropic(5.0, 25.0)),
    ("claude-opus-4-5", ModelRates::anthropic(5.0, 25.0)),
    ("claude-opus-4-1", ModelRates::anthropic(15.0, 75.0)),
    ("claude-opus-4-0", ModelRates::anthropic(15.0, 75.0)),
    ("claude-opus-4", ModelRates::anthropic(15.0, 75.0)),
    // Sonnet 5 still lists $2/$10; the old September expiry was incorrect.
    ("claude-sonnet-5-5", ModelRates::anthropic(2.0, 10.0)),
    ("claude-sonnet-5", ModelRates::anthropic(2.0, 10.0)),
    ("claude-sonnet-4-6", ModelRates::anthropic(3.0, 15.0)),
    ("claude-sonnet-4-5", ModelRates::anthropic(3.0, 15.0)),
    ("claude-sonnet-4-0", ModelRates::anthropic(3.0, 15.0)),
    ("claude-sonnet-4", ModelRates::anthropic(3.0, 15.0)),
    ("claude-haiku-4-5", ModelRates::anthropic(1.0, 5.0)),
    ("claude-haiku-3-5", ModelRates::anthropic(0.80, 4.0)),
    (
        "claude-fable-5-1",
        ModelRates::anthropic_with_cache_read(10.0, 50.0, 0.25),
    ),
    ("claude-fable-5", ModelRates::anthropic(10.0, 50.0)),
    (
        "claude-mythos-5-1",
        ModelRates::anthropic_with_cache_read(10.0, 50.0, 0.25),
    ),
    ("claude-mythos-5", ModelRates::anthropic(10.0, 50.0)),
    // OpenAI. Cache writes have no separate TTL tier.
    (
        "gpt-6.1-sol",
        ModelRates::openai_with_cache_write(2.00, 10.00, 0.10),
    ),
    (
        "gpt-6-astra",
        ModelRates::openai_with_cache_write(10.00, 50.00, 1.00),
    ),
    (
        "gpt-6-sol",
        ModelRates::openai_with_cache_write(2.00, 10.00, 0.20),
    ),
    (
        "gpt-6-luna",
        ModelRates::openai_with_cache_write(0.10, 0.50, 0.01),
    ),
    // Current Sol promotion is published through at least 2026-11-21.
    (
        "gpt-5.6-sol",
        ModelRates::openai_with_cache_write(4.00, 20.00, 0.40),
    ),
    (
        "gpt-5.6-terra",
        ModelRates::openai_with_cache_write(2.00, 12.00, 0.20),
    ),
    (
        "gpt-5.6-luna",
        ModelRates::openai_with_cache_write(0.20, 1.20, 0.02),
    ),
    (
        "gpt-5.6",
        ModelRates::openai_with_cache_write(4.00, 20.00, 0.40),
    ),
    (
        "gpt-5.6-cyber",
        ModelRates::openai_with_cache_write(12.50, 75.00, 1.25),
    ),
    ("gpt-5.5-cyber", ModelRates::openai(12.50, 75.00, 1.25)),
    // Pro models publish no cache discount: use full input for cache reads.
    ("gpt-5.5-pro", ModelRates::openai(30.00, 180.00, 30.00)),
    ("gpt-5.5", ModelRates::openai(5.00, 30.00, 0.50)),
    ("gpt-5.4-mini", ModelRates::openai(0.75, 4.50, 0.075)),
    ("gpt-5.4-nano", ModelRates::openai(0.20, 1.25, 0.02)),
    ("gpt-5.4-pro", ModelRates::openai(30.00, 180.00, 30.00)),
    ("gpt-5.4", ModelRates::openai(2.50, 15.00, 0.25)),
    ("gpt-5.3-codex", ModelRates::openai(1.75, 14.00, 0.175)),
    ("gpt-5.2-pro", ModelRates::openai(21.00, 168.00, 21.00)),
    ("gpt-5.2-codex", ModelRates::openai(1.75, 14.00, 0.175)),
    ("gpt-5.2", ModelRates::openai(1.75, 14.00, 0.175)),
    ("gpt-5.1-codex-max", ModelRates::openai(1.25, 10.00, 0.125)),
    ("gpt-5.1-codex-mini", ModelRates::openai(0.25, 2.00, 0.025)),
    ("gpt-5.1-codex", ModelRates::openai(1.25, 10.00, 0.125)),
    ("gpt-5.1", ModelRates::openai(1.25, 10.00, 0.125)),
    ("gpt-5-pro", ModelRates::openai(15.00, 120.00, 15.00)),
    ("gpt-5-nano", ModelRates::openai(0.05, 0.40, 0.005)),
    ("gpt-5-mini", ModelRates::openai(0.25, 2.00, 0.025)),
    ("gpt-5-codex", ModelRates::openai(1.25, 10.00, 0.125)),
    ("gpt-5", ModelRates::openai(1.25, 10.00, 0.125)),
    ("codex-mini-latest", ModelRates::openai(1.50, 6.00, 0.375)),
    ("gpt-4.1-nano", ModelRates::openai(0.10, 0.40, 0.025)),
    ("gpt-4.1-mini", ModelRates::openai(0.40, 1.60, 0.10)),
    ("gpt-4.1", ModelRates::openai(2.00, 8.00, 0.50)),
    ("gpt-4o-2024-05-13", ModelRates::openai(5.00, 15.00, 5.00)),
    ("gpt-4o-mini", ModelRates::openai(0.15, 0.60, 0.075)),
    ("gpt-4o", ModelRates::openai(2.50, 10.00, 1.25)),
    ("o4-mini", ModelRates::openai(1.10, 4.40, 0.275)),
    ("o3-pro", ModelRates::openai(20.00, 80.00, 20.00)),
    ("o3-mini", ModelRates::openai(1.10, 4.40, 0.55)),
    ("o3", ModelRates::openai(2.00, 8.00, 0.50)),
    ("o1-pro", ModelRates::openai(150.00, 600.00, 150.00)),
    ("o1", ModelRates::openai(15.00, 60.00, 7.50)),
];

/// Look up a verified model or dated snapshot. Unknown ids stay unpriced.
pub fn lookup(model_id: &str) -> Option<ModelRates> {
    let normalized = model_id.to_ascii_lowercase();
    let model = normalized.rsplit('/').next()?;
    let model = model.strip_prefix("anthropic.").unwrap_or(model);
    let model = match model.split_once('[') {
        Some((id, "1m]" | "200k]")) => id,
        Some(_) => return None,
        None => model,
    };
    RATES
        .iter()
        .find(|(known, _)| {
            model == *known
                || model
                    .strip_prefix(known)
                    .and_then(|suffix| suffix.strip_prefix('-'))
                    .is_some_and(is_snapshot_suffix)
        })
        .map(|(_, rates)| *rates)
}

fn is_snapshot_suffix(suffix: &str) -> bool {
    // Claude snapshots: YYYYMMDD (optionally Bedrock's -vN:M revision).
    // OpenAI snapshots: YYYY-MM-DD. No arbitrary tier/version suffixes.
    let date = match suffix.split_once("-v") {
        Some((date, revision)) => {
            let Some((version, variant)) = revision.split_once(':') else {
                return false;
            };
            if version.is_empty()
                || variant.is_empty()
                || !version.bytes().all(|b| b.is_ascii_digit())
                || !variant.bytes().all(|b| b.is_ascii_digit())
            {
                return false;
            }
            date
        }
        None => suffix,
    };
    (date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()))
        || (date.len() == 10
            && date.bytes().enumerate().all(|(i, b)| {
                if i == 4 || i == 7 {
                    b == b'-'
                } else {
                    b.is_ascii_digit()
                }
            }))
}

/// Convenience wrapper: price a token split for a possibly-unknown model.
pub fn cost_for(
    model_id: Option<&str>,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
) -> Option<f64> {
    cost_for_with_1h(model_id, input, output, cache_read, cache_write, 0)
}

/// As [`cost_for`], for a caller that can see the 1-hour cache-write
/// subset. See [`ModelRates::cost_usd_with_1h`].
pub fn cost_for_with_1h(
    model_id: Option<&str>,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    cache_write_1h: u64,
) -> Option<f64> {
    let rates = lookup(model_id?)?;
    Some(rates.cost_usd_with_1h(input, output, cache_read, cache_write, cache_write_1h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    #[test]
    fn matches_verified_claude_models() {
        // Dated snapshots and provider-prefixed catalogue ids preserve rates.
        for id in [
            "claude-opus-4-5-20251101",
            "claude-opus-4-6",
            "anthropic/claude-opus-4-5",
        ] {
            let rates = lookup(id).expect("opus should match");
            approx(rates.input, 5.0);
            approx(rates.output, 25.0);
        }
        approx(lookup("claude-sonnet-4-5").unwrap().input, 3.0);
        approx(lookup("claude-haiku-4-5").unwrap().input, 1.0);
        approx(lookup("claude-fable-5").unwrap().input, 10.0);
        approx(lookup("claude-mythos-5").unwrap().output, 50.0);
    }

    #[test]
    fn anthropic_cache_multipliers_are_derived_from_input() {
        let sonnet = lookup("claude-sonnet-4-5").unwrap();
        approx(sonnet.cache_read, 0.3); // 0.1x input
        approx(sonnet.cache_write, 3.75); // 1.25x input
    }

    #[test]
    fn cheaper_tiers_and_legacy_models_keep_their_own_rates() {
        approx(lookup("gpt-5-mini").unwrap().input, 0.25);
        approx(lookup("gpt-5.2-codex").unwrap().input, 1.75);
        approx(lookup("claude-opus-4-1-20250805").unwrap().input, 15.0);
    }

    #[test]
    fn current_openai_models_have_distinct_cache_rates() {
        for (id, input, output, cache_read) in [
            ("gpt-6.1-sol", 2.0, 10.0, 0.10),
            ("gpt-6-sol", 2.0, 10.0, 0.20),
            ("gpt-6-astra", 10.0, 50.0, 1.0),
            ("gpt-6-luna", 0.10, 0.50, 0.01),
            ("gpt-5.6-sol", 4.0, 20.0, 0.40),
            ("gpt-5.6-terra", 2.0, 12.0, 0.20),
            ("gpt-5.6-luna", 0.20, 1.20, 0.02),
            ("gpt-5.6-cyber", 12.50, 75.0, 1.25),
        ] {
            let rates = lookup(id).unwrap();
            approx(rates.input, input);
            approx(rates.output, output);
            approx(rates.cache_read, cache_read);
            approx(rates.cache_write, input * 1.25);
            approx(rates.cache_write_1h, rates.cache_write);
        }
        assert_eq!(lookup("gpt-5.6"), lookup("gpt-5.6-sol"));
    }

    #[test]
    fn newer_claude_models_override_family_cache_discount() {
        for (id, input, output, read) in [
            ("claude-opus-5-5", 4.0, 20.0, 0.20),
            ("claude-sonnet-5-5", 2.0, 10.0, 0.20),
            ("claude-fable-5-1", 10.0, 50.0, 0.25),
            ("claude-mythos-5-1", 10.0, 50.0, 0.25),
        ] {
            let rates = lookup(id).unwrap();
            approx(rates.input, input);
            approx(rates.output, output);
            approx(rates.cache_read, read);
            approx(
                rates.cost_usd_with_1h(0, 0, 1_000_000, 1_000_000, 500_000),
                read + input * 1.625,
            );
        }
        approx(lookup("claude-fable-5").unwrap().cache_read, 1.0);
    }

    #[test]
    fn provider_prefixes_and_snapshots_preserve_model_identity() {
        for id in [
            "anthropic/claude-opus-5-5[1m]",
            "anthropic.claude-opus-5-5-20260915-v1:0",
            "claude-opus-5-5-20260915",
        ] {
            assert_eq!(lookup(id), lookup("claude-opus-5-5"));
        }
        assert_eq!(
            lookup("openai/gpt-6.1-sol-2026-09-29"),
            lookup("gpt-6.1-sol")
        );
        approx(lookup("gpt-4o-2024-05-13").unwrap().input, 5.0);
        approx(lookup("gpt-4o-2024-08-06").unwrap().input, 2.5);
        approx(lookup("gpt-5.1-codex-mini").unwrap().input, 0.25);
    }

    #[test]
    fn future_versions_and_internal_labels_do_not_inherit_prices() {
        for id in [
            "gpt-5.7",
            "gpt-6.2-sol",
            "gpt-6-sol-pro",
            "gpt-5.6-unknown",
            "claude-opus-9-0",
            "claude-sonnet-5-6",
            "claude-fable-5-2",
            "codex-auto-review",
            "codex",
            "some-provider/not-gpt-5",
            "gpt-6-sol[unknown]",
            "claude-opus-5-5-20260915-vunknown",
        ] {
            assert!(lookup(id).is_none(), "{id} must remain unpriced");
        }
    }

    #[test]
    fn sonnet_5_keeps_its_current_published_rate() {
        // Sonnet 5 remains $2/$10 on 2026-10-01; Sonnet 4 is
        // $3/$15 and must not be dragged down with it.
        approx(lookup("claude-sonnet-5").unwrap().input, 2.0);
        approx(lookup("claude-sonnet-5").unwrap().output, 10.0);
        approx(lookup("claude-sonnet-4-6").unwrap().input, 3.0);
        // Haiku 3.5 is cheaper than the current family price.
        approx(lookup("claude-haiku-3-5").unwrap().input, 0.80);
        approx(lookup("claude-haiku-4-5").unwrap().input, 1.0);
    }

    /// Claude Code writes almost exclusively 1-hour cache entries, which
    /// bill at 2x input rather than the 5-minute tier's 1.25x.
    #[test]
    fn one_hour_cache_writes_bill_at_double_input() {
        let rates = lookup("claude-fable-5").unwrap();
        approx(rates.cache_write, 12.5); // 1.25x of $10
        approx(rates.cache_write_1h, 20.0); // 2x of $10

        // 1M cache-write tokens, all of it the 1-hour tier.
        approx(rates.cost_usd_with_1h(0, 0, 0, 1_000_000, 1_000_000), 20.0);
        // None of it — the old behavior, unchanged.
        approx(rates.cost_usd_with_1h(0, 0, 0, 1_000_000, 0), 12.5);
        approx(rates.cost_usd(0, 0, 0, 1_000_000), 12.5);
        // Half and half.
        approx(rates.cost_usd_with_1h(0, 0, 0, 1_000_000, 500_000), 16.25);
        // The 1-hour count is a SUBSET: over-reporting it cannot inflate
        // the bill beyond charging every written token at the 1h rate.
        approx(rates.cost_usd_with_1h(0, 0, 0, 1_000_000, 9_999_999), 20.0);
    }

    #[test]
    fn openai_models_have_no_separate_one_hour_tier() {
        let sol = lookup("gpt-5.6-sol").unwrap();
        approx(sol.cache_write, 5.0);
        approx(sol.cache_write_1h, 5.0);
        // Earlier generations have no separate published cache-write rate.
        approx(lookup("gpt-5").unwrap().cache_write, 0.0);
    }

    #[test]
    fn unknown_models_have_no_price() {
        assert!(lookup("kimi-k2").is_none());
        assert!(lookup("").is_none());
        assert!(cost_for(None, 1, 1, 1, 1).is_none());
        assert!(cost_for(Some("openrouter/some-new-thing"), 1000, 1000, 0, 0).is_none());
    }

    #[test]
    fn cost_is_a_dot_product_over_the_split() {
        let rates = ModelRates::anthropic(3.0, 15.0);
        // 1M input + 1M output + 1M cache-read + 1M cache-write, all of
        // the write at the 5-minute tier.
        let cost = rates.cost_usd(1_000_000, 1_000_000, 1_000_000, 1_000_000);
        approx(cost, 3.0 + 15.0 + 0.3 + 3.75);
        // Empty work is free, not NaN.
        approx(rates.cost_usd(0, 0, 0, 0), 0.0);
    }

    #[test]
    fn lookup_is_case_insensitive() {
        assert!(lookup("Claude-Sonnet-4-5").is_some());
        assert!(lookup("GPT-5-Codex").is_some());
    }
}
