import type {
  CategoryCost,
  FlatModelUsage,
  ProviderQuota,
  UsageSummary,
} from "@/tauri/commands";

/** The summary as if the hidden providers had never run.
 *
 *  Everything provider-scoped is rebuilt from the model rows, which carry
 *  their own token split, cost by type, cost sources, and savings inputs,
 *  so the filtered figures are exact rather than proportional guesses.
 *  Returns the summary itself when nothing it contains is hidden. */
export function filterUsageSummary(
  summary: UsageSummary,
  hidden: ReadonlySet<string>,
): UsageSummary {
  if (!summary.providers.some((p) => hidden.has(p.provider))) return summary;

  const shown = (provider: string) => !hidden.has(provider);
  const providers = summary.providers.filter((p) => shown(p.provider));
  const models = summary.models.filter((m) => shown(m.provider));
  const sum = (pick: (m: FlatModelUsage) => number) =>
    models.reduce((total, m) => total + pick(m), 0);

  const totalTokens = providers.reduce((n, p) => n + p.tokens, 0);
  const totalCost = providers.reduce((n, p) => n + p.cost_usd, 0);
  const cacheRead = sum((m) => m.cache_read_tokens);
  const input = sum((m) => m.input_tokens);
  const cacheWrite = sum((m) => m.cache_write_tokens);
  const observedInput = input + cacheRead + cacheWrite;
  const savings = sum((m) => m.cache_savings.savings_usd);
  const savingsBasis = sum((m) => m.cache_savings.cached_usd);
  const sourcedCost =
    sum((m) => m.cost_sources.provider_usd) +
    sum((m) => m.cost_sources.table_usd) +
    sum((m) => m.cost_sources.override_usd);
  const costShare = (part: number) => (sourcedCost > 0 ? part / sourcedCost : 0);

  const categoryCost: CategoryCost = {
    input: sum((m) => m.category_cost.input),
    output: sum((m) => m.category_cost.output),
    cache_read: sum((m) => m.category_cost.cache_read),
    cache_write: sum((m) => m.category_cost.cache_write),
    unsplit: sum((m) => m.category_cost.unsplit),
  };

  return {
    ...summary,
    buckets: summary.buckets.map((bucket) => ({
      ...bucket,
      providers: Object.fromEntries(
        Object.entries(bucket.providers).filter(([provider]) => shown(provider)),
      ),
    })),
    providers,
    models,
    totals: {
      estimated_cost_usd: totalCost,
      total_tokens: totalTokens,
      cache_read_share: totalTokens > 0 ? cacheRead / totalTokens : 0,
      // A session belongs to one provider, so per-provider counts add up.
      session_count: providers.reduce((n, p) => n + p.session_count, 0),
    },
    composition: {
      processed_tokens: totalTokens,
      cache_read_tokens: cacheRead,
      cache_read_share_of_input: observedInput > 0 ? cacheRead / observedInput : 0,
      input_tokens: input,
      cache_write_tokens: cacheWrite,
      output_tokens: sum((m) => m.output_tokens),
      reasoning_tokens: sum((m) => m.reasoning_tokens),
      cache_savings_usd: savings,
      cache_savings_multiplier:
        savingsBasis > 0.005 ? sum((m) => m.cache_savings.uncached_usd) / savingsBasis : null,
    },
    category_cost: categoryCost,
    confidence: {
      provider_reported_share: costShare(sum((m) => m.cost_sources.provider_usd)),
      table_priced_share: costShare(sum((m) => m.cost_sources.table_usd)),
      override_priced_share: costShare(sum((m) => m.cost_sources.override_usd)),
      unpriced_token_share: totalTokens > 0 ? sum((m) => m.unpriced_tokens) / totalTokens : 0,
      cache_savings_usd: savings,
    },
    quota: filterQuota(summary.quota, hidden),
    accounts: summary.accounts?.filter((a) => shown(a.provider)),
  };
}

export function filterQuota(
  quota: Record<string, ProviderQuota>,
  hidden: ReadonlySet<string>,
): Record<string, ProviderQuota> {
  return Object.fromEntries(
    Object.entries(quota).filter(([key]) => !hidden.has(parseQuotaKey(key).provider)),
  );
}

/** Split a quota key into its provider and account: `claude` is the CLI's
 *  own sign-in, `claude@<id>` an extra provider account. */
export function parseQuotaKey(key: string): { provider: string; accountId: string } {
  const at = key.indexOf("@");
  return at < 0
    ? { provider: key, accountId: "default" }
    : { provider: key.slice(0, at), accountId: key.slice(at + 1) };
}
