//! Static list-price table for the usage ledger.
//!
//! The Settings → Usage dashboard reports what a turn *would* cost at
//! published list prices, regardless of how the user actually pays for it
//! (a Claude Max subscription, a ChatGPT plan, or metered API keys). It is an
//! API-equivalent estimate, not a claim about what a plan covered or what the
//! provider billed.
//!
//! Matching is by **model-id substring**, deliberately: every provider
//! spells its ids differently and they gain suffixes over time
//! (`claude-opus-4-5-20251101`, `gpt-5.2-codex`, the OpenCode catalogue's
//! `anthropic/claude-sonnet-4-5`). A substring table degrades to "unknown
//! model" rather than to a wrong price, and an unknown model contributes
//! tokens but no cost — the UI shows the tokens and simply omits them from
//! the money column.
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
    /// Anthropic's cache multipliers are uniform across the model line:
    /// a 5-minute cache write costs 1.25x base input, a 1-hour write 2x,
    /// and a cache read 0.1x. Expressing them as a constructor keeps the
    /// table below honest — a future price change touches one number per
    /// model, not five. Re-verified against the published rate card on
    /// 2026-08-08: every listed model is a clean multiple of its input.
    const fn anthropic(input: f64, output: f64) -> Self {
        Self {
            input,
            output,
            cache_read: input * 0.1,
            cache_write: input * 1.25,
            cache_write_1h: input * 2.0,
        }
    }

    /// OpenAI-style pricing, where the cached-input rate is published
    /// directly rather than derived, and no separate charge for writing
    /// to the cache is published for this model.
    ///
    /// `cache_write: 0.0` is a statement about the published rate card,
    /// not a guess: outside the GPT-5.6 generation OpenAI does not list a
    /// cache-write price at all, and inventing one would put a number in
    /// the money column that no document supports.
    const fn openai(input: f64, output: f64, cache_read: f64) -> Self {
        Self {
            input,
            output,
            cache_read,
            cache_write: 0.0,
            cache_write_1h: 0.0,
        }
    }

    /// GPT-5.6-generation pricing, which *does* publish a cache-write
    /// rate: 1.25x uncached input, with cached input at 0.1x. OpenAI
    /// publishes no longer-TTL tier, so both write rates are the same.
    const fn openai_5_6(input: f64, output: f64) -> Self {
        Self {
            input,
            output,
            cache_read: input * 0.1,
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

/// The whole price list, most-specific pattern first.
///
/// Order matters: `lookup` returns the first match, so a narrower id
/// fragment (`opus-4-1`) has to precede the family catch-all (`opus`).
/// Keeping every rate in this one array is intentional — a price change is
/// a one-line diff here and nowhere else.
/// Sources, both re-read and re-verified **2026-08-08**:
///
/// - Anthropic: <https://platform.claude.com/docs/en/docs/about-claude/pricing>
///   (the canonical target of the old `docs.anthropic.com` URL).
/// - OpenAI: <https://developers.openai.com/api/docs/pricing> (the
///   canonical 301 target of `platform.openai.com/docs/pricing`) plus the
///   per-model pages under `developers.openai.com/api/docs/models/<id>`.
///   `openai.com/api/pricing` is bot-gated and returns 403.
const RATES: &[(&str, ModelRates)] = &[
    // ── Anthropic (Claude) ──
    ("opus-4-1", ModelRates::anthropic(15.0, 75.0)),
    ("opus-4-0", ModelRates::anthropic(15.0, 75.0)),
    ("opus", ModelRates::anthropic(5.0, 25.0)),
    // Sonnet 5 launched on an introductory rate and reverts to the
    // family price on **2026-09-01**. Dated deliberately: after that
    // date this line should be deleted so `sonnet` catches it again.
    ("sonnet-5", ModelRates::anthropic(2.0, 10.0)),
    ("sonnet", ModelRates::anthropic(3.0, 15.0)),
    // Haiku 3.5 is cheaper than the current family price; it is retired
    // on first-party but still served on Bedrock/Vertex, and old ledger
    // rows referencing it must not be re-priced upward.
    ("haiku-3-5", ModelRates::anthropic(0.80, 4.0)),
    ("haiku", ModelRates::anthropic(1.0, 5.0)),
    ("fable", ModelRates::anthropic(10.0, 50.0)),
    ("mythos", ModelRates::anthropic(10.0, 50.0)),
    // ── OpenAI (Codex / GPT) ──
    //
    // ORDERING IS LOad-BEARING and the reason this block is long. The
    // lookup is first-match-wins over substrings, so every generation
    // that is *not* $1.25/$10 has to appear before the `gpt-5` catch-all
    // — otherwise `gpt-5.6-sol` (the model the Codex CLI actually runs)
    // is billed at a quarter of its real input rate. Tier suffixes
    // (`-pro`, `-mini`, `-nano`) precede their own family for the same
    // reason, and every `gpt-5.x-codex` id precedes the generic `codex`
    // entry because those ids contain both fragments.
    //
    // GPT-5.6 is three separately-priced models, not three effort
    // levels: sol (flagship), terra (balanced), luna (high-volume).
    // Bare `gpt-5.6` is an alias that routes to sol.
    ("gpt-5.6-sol", ModelRates::openai_5_6(5.00, 30.00)),
    ("gpt-5.6-terra", ModelRates::openai_5_6(2.00, 12.00)),
    ("gpt-5.6-luna", ModelRates::openai_5_6(0.20, 1.20)),
    ("gpt-5.6", ModelRates::openai_5_6(5.00, 30.00)),
    // The `-pro` tiers publish no cached-input rate, so a cache read is
    // priced at the full input rate rather than at an invented discount.
    ("gpt-5.5-pro", ModelRates::openai(30.00, 180.00, 30.00)),
    ("gpt-5.5", ModelRates::openai(5.00, 30.00, 0.50)),
    ("gpt-5.4-mini", ModelRates::openai(0.75, 4.50, 0.075)),
    ("gpt-5.4-nano", ModelRates::openai(0.20, 1.25, 0.02)),
    ("gpt-5.4-pro", ModelRates::openai(30.00, 180.00, 30.00)),
    ("gpt-5.4", ModelRates::openai(2.50, 15.00, 0.25)),
    ("gpt-5.3-codex", ModelRates::openai(1.75, 14.00, 0.175)),
    ("gpt-5.2-pro", ModelRates::openai(21.00, 168.00, 21.00)),
    ("gpt-5.2", ModelRates::openai(1.75, 14.00, 0.175)),
    ("gpt-5.1", ModelRates::openai(1.25, 10.00, 0.125)),
    ("gpt-5-pro", ModelRates::openai(15.00, 120.00, 15.00)),
    ("gpt-5-nano", ModelRates::openai(0.05, 0.40, 0.005)),
    ("gpt-5-mini", ModelRates::openai(0.25, 2.00, 0.025)),
    ("gpt-5-codex", ModelRates::openai(1.25, 10.00, 0.125)),
    ("gpt-5", ModelRates::openai(1.25, 10.00, 0.125)),
    ("codex-mini", ModelRates::openai(1.50, 6.00, 0.375)),
    ("codex", ModelRates::openai(1.25, 10.00, 0.125)),
    ("gpt-4.1-mini", ModelRates::openai(0.40, 1.60, 0.10)),
    ("gpt-4.1", ModelRates::openai(2.00, 8.00, 0.50)),
    ("gpt-4o-mini", ModelRates::openai(0.15, 0.60, 0.075)),
    ("gpt-4o", ModelRates::openai(2.50, 10.00, 1.25)),
    ("o4-mini", ModelRates::openai(1.10, 4.40, 0.275)),
    ("o3", ModelRates::openai(2.00, 8.00, 0.50)),
];

/// Look up list prices for `model_id`, or `None` when the id matches
/// nothing in the table.
///
/// `None` is a first-class answer, not a failure: the ledger still records
/// the token split for an unrecognized model, and the dashboard renders it
/// with an empty cost cell rather than inventing a number.
pub fn lookup(model_id: &str) -> Option<ModelRates> {
    let needle = model_id.to_ascii_lowercase();
    RATES
        .iter()
        .find(|(pattern, _)| needle.contains(pattern))
        .map(|(_, rates)| *rates)
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
    fn matches_claude_families_by_substring() {
        // Dated snapshots, bare aliases, and provider-prefixed catalogue
        // ids all have to land on the same family.
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
    fn more_specific_patterns_win() {
        // `gpt-5-mini` must not be swallowed by the `gpt-5` family entry,
        // and `opus-4-1` must not be swallowed by `opus`.
        approx(lookup("gpt-5-mini").unwrap().input, 0.25);
        approx(lookup("gpt-5.2-codex").unwrap().input, 1.75);
        approx(lookup("claude-opus-4-1-20250805").unwrap().input, 15.0);
    }

    /// The regression this table was rewritten for. The Codex CLI runs
    /// `gpt-5.6-sol`, which is $5/$30 — the generic `gpt-5` entry prices
    /// it at $1.25/$10, understating real spend fourfold. Ordering is the
    /// only thing that prevents it, so assert on the ordering directly.
    #[test]
    fn gpt_5_6_variants_are_not_swallowed_by_the_gpt_5_family() {
        let sol = lookup("gpt-5.6-sol").expect("sol is priced");
        approx(sol.input, 5.0);
        approx(sol.output, 30.0);
        approx(sol.cache_read, 0.5);
        let terra = lookup("gpt-5.6-terra").unwrap();
        approx(terra.input, 2.0);
        approx(terra.output, 12.0);
        let luna = lookup("gpt-5.6-luna").unwrap();
        approx(luna.input, 0.20);
        approx(luna.output, 1.20);
        // The bare alias routes to sol.
        approx(lookup("gpt-5.6").unwrap().input, 5.0);
        // And the generic entry is still there for the ids that really
        // are $1.25/$10.
        approx(lookup("gpt-5").unwrap().input, 1.25);
        approx(lookup("gpt-5-codex").unwrap().input, 1.25);
        approx(lookup("gpt-5.1").unwrap().input, 1.25);

        // Ordering is load-bearing: every non-$1.25 OpenAI pattern must
        // appear before the `gpt-5` catch-all, or it never matches.
        let index = |needle: &str| {
            RATES
                .iter()
                .position(|(pattern, _)| *pattern == needle)
                .unwrap_or_else(|| panic!("{needle} missing from the table"))
        };
        let catch_all = index("gpt-5");
        for specific in [
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "gpt-5.6",
            "gpt-5.5-pro",
            "gpt-5.5",
            "gpt-5.4",
            "gpt-5.3-codex",
            "gpt-5.2",
            "gpt-5-pro",
            "gpt-5-mini",
            "gpt-5-nano",
        ] {
            assert!(
                index(specific) < catch_all,
                "{specific} must precede the gpt-5 catch-all"
            );
        }
        // `gpt-5.x-codex` ids contain BOTH fragments, so they must also
        // beat the generic `codex` entry.
        assert!(index("gpt-5.3-codex") < index("codex"));
        assert!(index("codex-mini") < index("codex"));
    }

    #[test]
    fn sonnet_5_holds_its_introductory_rate() {
        // Sonnet 5 is $2/$10 until 2026-09-01; the rest of the family is
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
        approx(sol.cache_write, 6.25);
        approx(sol.cache_write_1h, 6.25);
        // No cache-write rate is published outside the 5.6 generation.
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
