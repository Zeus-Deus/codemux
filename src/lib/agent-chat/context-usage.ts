/**
 * Context-window usage — derivation + display formatting.
 *
 * The provider reports raw token counts (`ContextUsageSnapshot`); this
 * module turns them into the handful of display-ready values the
 * composer's meter renders. Kept pure and UI-free so the banding rules
 * (when a percentage exists, when the lifetime total is worth showing)
 * are testable in one place and identical between the ring, the bar,
 * and the popup rows.
 *
 * Design rule throughout: never invent a denominator. A snapshot with
 * no `max_tokens` and no capability seed yields `usedPercentage: null`,
 * and every surface degrades to a bare token count rather than showing
 * a made-up fraction of a guessed window.
 */
import type { ContextUsageSnapshot } from "@/tauri/events";
import type { ChatModelInfo } from "@/tauri/types";

/** Drop `NaN` / `Infinity` / `null` / `undefined` to a single `null`. */
function finite(value: number | null | undefined): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

/** Same, but also rejects zero and negatives — used for denominators
 *  and window sizes, where a non-positive value is as unusable as an
 *  absent one. */
function positive(value: number | null | undefined): number | null {
  const n = finite(value);
  return n !== null && n > 0 ? n : null;
}

/** Trim a trailing `.0` so `"1.0k"` reads as `"1k"`. */
function trimDecimal(text: string): string {
  return text.endsWith(".0") ? text.slice(0, -2) : text;
}

/**
 * Compact token count for dense UI: `"999"`, `"1.4k"`, `"224k"`,
 * `"3.1m"`.
 *
 * Under 10k keeps one decimal because the difference between 1.4k and
 * 2.3k of context matters at that scale; above it the decimal is noise.
 *
 * The band is chosen from the *rounded* figure, not the raw one: 999_600
 * is below 1m but rounds to 1000 thousands, and `"1000k"` is a unit that
 * doesn't exist in this scheme — it promotes to `"1m"` instead.
 *
 * Never returns an empty string or a dash — an unknown count formats as
 * `"0"` so the composer's fixed-width row can't jump when the first
 * real reading lands.
 */
export function formatContextTokens(value: number | null): string {
  const n = finite(value);
  if (n === null || n <= 0) return "0";
  if (n < 1_000) return String(Math.round(n));
  if (n < 10_000) return `${trimDecimal((n / 1_000).toFixed(1))}k`;
  const thousands = Math.round(n / 1_000);
  if (thousands < 1_000) return `${thousands}k`;
  return `${trimDecimal((n / 1_000_000).toFixed(1))}m`;
}

/**
 * Percentage for display: one decimal below 10% (where whole numbers
 * would spend the first tenth of the window reading a flat "0%"),
 * integer at or above it.
 *
 * Returns `null` — not `"0%"` — when there is no percentage to show, so
 * callers can distinguish "unknown window" from "empty window" and hide
 * the bar entirely in the former case.
 */
export function formatContextPercentage(value: number | null): string | null {
  const n = finite(value);
  if (n === null) return null;
  if (n < 10) return `${trimDecimal(n.toFixed(1))}%`;
  return `${Math.round(n)}%`;
}

/**
 * Best-effort context-window size from the capability registry, used to
 * paint the meter before the provider reports its own `max_tokens`.
 *
 * Cascade: the selected context-window option's numeric size, then the
 * option marked default, then the model's own advertised window. A
 * snapshot's `max_tokens` always outranks this — see
 * {@link deriveContextUsageDisplay}.
 */
export function resolveContextWindowTokens(
  model: ChatModelInfo | null | undefined,
  selectedContextWindow: string | null | undefined,
): number | null {
  const options = model?.context_window_options ?? [];
  if (selectedContextWindow) {
    const selected = options.find((o) => o.value === selectedContextWindow);
    const tokens = positive(selected?.context_window_tokens);
    if (tokens !== null) return tokens;
  }
  const fallback = options.find((o) => o.is_default);
  const fallbackTokens = positive(fallback?.context_window_tokens);
  if (fallbackTokens !== null) return fallbackTokens;
  return positive(model?.max_context_tokens);
}

export interface ContextUsageDisplay {
  /** Live occupancy. `0` when unknown — never null, so the readout
   *  always has something to print. */
  usedTokens: number;
  /** Window size, or `null` when neither the provider nor the
   *  capability seed knows it. `null` hides the bar and the percent. */
  maxTokens: number | null;
  /** `0`–`100`, clamped. `null` whenever `maxTokens` is. */
  usedPercentage: number | null;
  /** Lifetime processed total, but only when it actually exceeds live
   *  occupancy — i.e. only once compaction has discarded something.
   *  Before that it would just restate `usedTokens`. */
  totalProcessedTokens: number | null;
  /** Whether the provider compacts on its own, which is the difference
   *  between a full window being a warning and being a non-event. */
  compactsAutomatically: boolean;
}

/**
 * Fold a snapshot plus the capability seed into display-ready values.
 *
 * `seedMaxTokens` only fills in for a snapshot that has no `max_tokens`
 * of its own — the runtime report is authoritative, since the effective
 * window can differ from the advertised one (beta headers, per-account
 * limits, federated providers).
 */
export function deriveContextUsageDisplay(
  snapshot: ContextUsageSnapshot | null | undefined,
  seedMaxTokens?: number | null,
): ContextUsageDisplay {
  const usedRaw = finite(snapshot?.used_tokens);
  const usedTokens = usedRaw !== null && usedRaw > 0 ? usedRaw : 0;
  const maxTokens = positive(snapshot?.max_tokens) ?? positive(seedMaxTokens);
  const usedPercentage =
    snapshot && maxTokens !== null
      ? Math.min(100, Math.max(0, (usedTokens / maxTokens) * 100))
      : null;
  const totalRaw = finite(snapshot?.total_processed_tokens);
  const totalProcessedTokens =
    totalRaw !== null && totalRaw > usedTokens ? totalRaw : null;
  return {
    usedTokens,
    maxTokens,
    usedPercentage,
    totalProcessedTokens,
    compactsAutomatically: snapshot?.compacts_automatically === true,
  };
}
