import { useCallback, useEffect, useMemo, useState } from "react";

import { ChevronDown, ChevronUp, Loader2, RefreshCw } from "lucide-react";

import { Button } from "@/components/ui/button";
import { ProviderLogo } from "@/components/chat/provider-logo";
import { SegmentedControl } from "./settings-primitives";
import {
  usageExportCsv,
  usageScanProviderHistory,
  usageSummary,
} from "@/tauri/commands";
import type {
  CostConfidence,
  FlatModelUsage,
  PlanUsageWindow,
  UsageComposition,
  PlanWindowKind,
  ProviderQuota,
  UsageBucket,
  UsagePeriod,
  UsageProvider,
  UsageSummary,
} from "@/tauri/commands";
import type { AgentChatProviderKind } from "@/tauri/types";
import { toast } from "@/lib/toast";
import { cn } from "@/lib/utils";

/** How often to re-poll while the page is open. The ledger only grows
 *  when an agent is mid-turn, so this is about keeping an open settings
 *  tab honest, not about being live. */
const POLL_MS = 30_000;

const PERIOD_OPTIONS: { value: UsagePeriod; label: string }[] = [
  { value: "today", label: "Today" },
  { value: "7d", label: "7 days" },
  { value: "30d", label: "30 days" },
  { value: "90d", label: "90 days" },
];

type Metric = "cost" | "tokens";

const METRIC_OPTIONS: { value: Metric; label: string }[] = [
  { value: "cost", label: "Est. cost" },
  { value: "tokens", label: "Tokens" },
];

/** Series colors, per the design-system token rules — no raw palette
 *  classes. Claude takes the brand accent, OpenCode the green status
 *  tone, and Codex a neutral foreground tint (Codemux has no third
 *  brand hue, and inventing one would imply a status meaning). */
const SERIES_FILL: Record<string, string> = {
  claude: "bg-accent-ember",
  codex: "bg-foreground/45",
  opencode: "bg-status-open",
};

const SERIES_LABEL: Record<string, string> = {
  claude: "Claude Code",
  codex: "Codex",
  opencode: "OpenCode",
};

/** Fallback for a provider id the frontend does not know — the ledger
 *  outlives the provider list, so a row from a since-removed adapter
 *  must still render rather than crash. */
const UNKNOWN_FILL = "bg-muted-foreground/40";

function seriesFill(provider: string): string {
  return SERIES_FILL[provider] ?? UNKNOWN_FILL;
}

function seriesLabel(provider: string): string {
  return SERIES_LABEL[provider] ?? provider;
}

function isKnownProvider(provider: string): provider is AgentChatProviderKind {
  return provider === "claude" || provider === "codex" || provider === "opencode";
}

// ── formatting ──

/** 8.0B / 1.4M / 82K / 640 — the design's `toks`.
 *
 *  The B tier is not hypothetical: provider history on a busy machine
 *  puts the 30-day figure in the billions, and without it the hero read
 *  "7961.5M" — technically correct, unreadable, and visibly wider than
 *  the stat next to it. */
export function formatTokens(n: number): string {
  if (!Number.isFinite(n)) return "0";
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)}B`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${Math.round(n / 1e3)}K`;
  return String(Math.round(n));
}

/** Thousands separators keep large API-equivalent estimates readable. */
const MONEY = new Intl.NumberFormat("en-US", {
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
});
/** $12.34 / $36,999.88 — an API/list-price equivalent, not an invoice. */
export function formatMoney(n: number): string {
  return `$${MONEY.format(Number.isFinite(n) ? n : 0)}`;
}

/** Short name for each quota window, for the meter's row label. */
const WINDOW_LABEL: Record<PlanWindowKind, string> = {
  five_hour: "5h",
  seven_day: "week",
  seven_day_opus: "opus",
  seven_day_sonnet: "sonnet",
  overage: "extra",
  other: "limit",
};

/** Tone thresholds from the design: near the cap reads as attention,
 *  well into the window as ember, otherwise calm. Status tokens only —
 *  no raw palette classes. */
export function meterTone(usedPct: number): string {
  if (usedPct >= 85) return "bg-status-attention";
  if (usedPct >= 60) return "bg-accent-ember";
  return "bg-status-open";
}

/** "resets 16:40" in the viewer's local time; empty when unknown. */
export function formatResetAt(resetsAtMs: number | null | undefined): string {
  if (resetsAtMs == null || !Number.isFinite(resetsAtMs)) return "";
  const when = new Date(resetsAtMs);
  if (Number.isNaN(when.getTime())) return "";
  return `resets ${when.getHours()}:${String(when.getMinutes()).padStart(2, "0")}`;
}

/** The (at most two) windows a lane shows as bars.
 *
 *  Claude reports per-model weekly windows (`seven_day_opus` /
 *  `seven_day_sonnet`) alongside the overall `seven_day`. Showing all of
 *  them would turn a two-bar lane into a stack, so the bars stay
 *  5h + overall-weekly and the per-model ones move into the note. */
export function meterWindows(windows: PlanUsageWindow[]): PlanUsageWindow[] {
  const fiveHour = windows.find((w) => w.kind === "five_hour");
  const weekly =
    windows.find((w) => w.kind === "seven_day") ??
    // No overall weekly? Fall back to whichever per-model one is highest,
    // so the lane still shows the binding constraint.
    windows
      .filter(
        (w) => w.kind === "seven_day_opus" || w.kind === "seven_day_sonnet",
      )
      .sort((a, b) => b.used_pct - a.used_pct)[0];
  return [fiveHour, weekly].filter(Boolean) as PlanUsageWindow[];
}

/** The line under the meters: reset time, plus any per-model weekly
 *  windows that did not get their own bar. */
export function meterNote(quota: ProviderQuota): string {
  const bars = meterWindows(quota.windows);
  const parts: string[] = [];
  const reset = formatResetAt(bars[0]?.resets_at_ms);
  if (reset) parts.push(`5h ${reset}`);
  const perModel = quota.windows.filter(
    (w) => w.kind === "seven_day_opus" || w.kind === "seven_day_sonnet",
  );
  const shown = new Set(bars.map((w) => w.kind));
  const extra = perModel.filter((w) => !shown.has(w.kind));
  if (extra.length > 0) {
    parts.push(
      extra
        .map((w) => `${WINDOW_LABEL[w.kind]} ${Math.round(w.used_pct)}%`)
        .join(" · "),
    );
  }
  return parts.join(" · ");
}

function formatPercent(fraction: number): string {
  return `${Math.round((Number.isFinite(fraction) ? fraction : 0) * 100)}%`;
}

/**
 * "Usage" settings section — token and cost accounting per provider,
 * model, and session, read from the local `agent_usage_ledger`.
 *
 * Cost is always an API/list-price equivalent. Subscription quota is shown
 * separately when a provider reports it; the page never guesses what was
 * actually billed.
 */
export function UsageSection() {
  const [period, setPeriod] = useState<UsagePeriod>("7d");
  const [metric, setMetric] = useState<Metric>("cost");
  const [summary, setSummary] = useState<UsageSummary | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [hovered, setHovered] = useState<number | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [exporting, setExporting] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [breakdown, setBreakdown] = useState<BreakdownView>("model");
  const [scanning, setScanning] = useState(false);
  /// Whether the open-time provider-history scan has settled.
  const [scanned, setScanned] = useState(false);

  const refresh = useCallback(() => {
    setError(null);
    setRefreshing(true);
    return usageSummary(period)
      .then((next) => setSummary(next))
      .catch((err) => {
        setSummary(null);
        setError(String(err));
      })
      .finally(() => setRefreshing(false));
  }, [period]);

  const scan = useCallback(async () => {
    setScanning(true);
    try {
      await usageScanProviderHistory();
    } catch (err) {
      toast.error("Could not read provider history", { description: String(err) });
    } finally {
      setScanning(false);
    }
  }, []);

  /// Scan first, then refetch — the summary must see the imported rows.
  const scanThenRefresh = useCallback(async () => {
    await scan();
    await refresh();
  }, [scan, refresh]);

  // A scan on page open keeps provider history current. The scan is
  // incremental, so unchanged sources are cheap.
  //
  // Deliberately mount-only: a period change cannot affect source data.
  useEffect(() => {
    void scan().finally(() => setScanned(true));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // The first fetch waits for that scan. Firing both at once would cost
  // two round-trips on every open and briefly render pre-import figures
  // that then jump — on this machine that is the difference between a
  // hero reading millions and one reading billions.
  useEffect(() => {
    if (!scanned) return;
    refresh();
  }, [refresh, scanned]);

  // Poll the histories too: without runtime accounting, refreshing only the
  // summary would leave an open page stale while a provider app is active.
  useEffect(() => {
    const id = window.setInterval(() => {
      void scanThenRefresh();
    }, POLL_MS);
    return () => window.clearInterval(id);
  }, [scanThenRefresh]);

  // A hovered bucket index is only meaningful for the period it was
  // taken in — a stale index would read out the wrong bar.
  useEffect(() => {
    setHovered(null);
  }, [period]);

  const handleExport = async () => {
    setExporting(true);
    try {
      const csv = await usageExportCsv(period);
      const blob = new Blob([csv], { type: "text/csv;charset=utf-8" });
      const url = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = url;
      link.download = `codemux-usage-${period}.csv`;
      link.click();
      URL.revokeObjectURL(url);
    } catch (err) {
      toast.error("Export failed", { description: String(err) });
    } finally {
      setExporting(false);
    }
  };

  return (
    <div>
      <div className="mb-6 flex items-end justify-between gap-4">
        <div className="min-w-0">
          <h2 className="text-base font-semibold tracking-tight">Usage</h2>
          <p className="mt-1 text-[12px] text-muted-foreground">
            {summary ? rangeLabel(summary) : "Loading…"} ·{" "}
            {refreshing ? "refreshing…" : "live"}
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <SegmentedControl
            value={period}
            onChange={setPeriod}
            options={PERIOD_OPTIONS}
            ariaLabel="Usage period"
          />
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label="Refresh usage"
            onClick={() => scanThenRefresh()}
            disabled={refreshing || scanning}
          >
            <RefreshCw
              className={cn(
                "h-3.5 w-3.5",
                (refreshing || scanning) && "animate-spin",
              )}
              aria-hidden
            />
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={handleExport}
            disabled={exporting || !summary}
          >
            Export CSV
          </Button>
        </div>
      </div>

      {error && (
        <p className="mb-4 rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive">
          Failed to load usage: {error}
        </p>
      )}

      {summary === null ? (
        !error && (
          <div className="flex items-center gap-2 py-6 text-sm text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" aria-hidden />
            Loading usage…
          </div>
        )
      ) : summary.totals.total_tokens === 0 ? (
        <div className="rounded-lg border border-border/60 bg-muted/30 px-4 py-10 text-center text-[13px] text-muted-foreground">
          No agent activity in this period.
        </div>
      ) : (
        <div className="space-y-4">
          <OverviewCard
            summary={summary}
            metric={metric}
            onMetricChange={setMetric}
            hovered={hovered}
            onHover={setHovered}
          />
          <CompositionRow composition={summary.composition} />
          <LanesCard
            summary={summary}
            expanded={expanded}
            onToggle={(provider) =>
              setExpanded((current) => (current === provider ? null : provider))
            }
          />
          <BreakdownCard
            summary={summary}
            view={breakdown}
            onViewChange={setBreakdown}
          />
          <ProviderHistoryFooter busy={scanning} sessionCount={summary.totals.session_count} />
        </div>
      )}
    </div>
  );
}

function rangeLabel(summary: UsageSummary): string {
  if (summary.period === "today") return "Today, 00:00 – now";
  const first = summary.buckets[0];
  const last = summary.buckets[summary.buckets.length - 1];
  if (!first || !last) return "";
  return `${first.sub_label} – ${last.sub_label}`;
}

/** The value one bucket contributes for one provider, under the active
 *  metric. Shared by the chart and the readout so a hovered bar and its
 *  numbers can never disagree. */
function bucketValue(
  bucket: UsageBucket,
  provider: string,
  metric: Metric,
): number {
  const slice = bucket.providers[provider];
  if (!slice) return 0;
  return metric === "cost" ? slice.cost_usd : slice.tokens;
}

function formatMetric(value: number, metric: Metric): string {
  return metric === "cost" ? formatMoney(value) : formatTokens(value);
}

// ── overview ──

/** Design canvas value. The chart flexes horizontally on its own; only
 *  the height needs stating. */
const CHART_HEIGHT_PX = 150;

function OverviewCard({
  summary,
  metric,
  onMetricChange,
  hovered,
  onHover,
}: {
  summary: UsageSummary;
  metric: Metric;
  onMetricChange: (metric: Metric) => void;
  hovered: number | null;
  onHover: (index: number | null) => void;
}) {
  const { totals, providers, buckets } = summary;
  // Lane order drives series order everywhere — legend, stack, and
  // readout — so the eye can track one provider across all three.
  const order = useMemo(() => providers.map((p) => p.provider), [providers]);

  const max = useMemo(() => {
    const totalsPerBucket = buckets.map((bucket) =>
      order.reduce((sum, p) => sum + bucketValue(bucket, p, metric), 0),
    );
    return Math.max(...totalsPerBucket, 0) || 1;
  }, [buckets, order, metric]);

  const hoveredBucket = hovered === null ? null : (buckets[hovered] ?? null);

  const readoutValue = hoveredBucket
    ? formatMetric(
        order.reduce((sum, p) => sum + bucketValue(hoveredBucket, p, metric), 0),
        metric,
      )
    : metric === "cost"
      ? formatMoney(totals.estimated_cost_usd)
      : formatTokens(totals.total_tokens);

  const readoutBreakdown = hoveredBucket
    ? order
        .map(
          (p) =>
            `${seriesLabel(p).split(" ")[0]} ${formatMetric(
              bucketValue(hoveredBucket, p, metric),
              metric,
            )}`,
        )
        .join("   ")
    : metric === "cost"
      ? "API/list-price equivalent"
      : "input + output + cache read + cache write";

  return (
    <div className="rounded-lg border border-border/60 bg-muted/30 p-4">
      <div className="flex items-end justify-between gap-4">
        {/* Gaps and stat size step up to the design's values (44px / 27px)
            only at the viewport where the settings column actually goes
            wide; below that they stay at the compact figures the reading
            column needs. */}
        <div className="flex flex-wrap items-end gap-x-8 gap-y-4 xl:gap-x-11">
          <HeroStat
            label="Estimated cost"
            value={formatMoney(totals.estimated_cost_usd)}
            note="API/list-price equivalent"
          />
          <HeroStat
            label="Tokens"
            value={formatTokens(totals.total_tokens)}
            note={`${formatPercent(totals.cache_read_share)} served from cache`}
          />
          <HeroStat
            label="Sessions"
            value={totals.session_count.toLocaleString()}
            note="provider history on this machine"
          />
        </div>
        <div className="shrink-0">
          <SegmentedControl
            value={metric}
            onChange={onMetricChange}
            options={METRIC_OPTIONS}
            ariaLabel="Chart metric"
            size="sm"
          />
        </div>
      </div>

      <div className="mt-4 flex flex-wrap items-baseline gap-x-3 gap-y-1 border-t border-border/40 pt-3">
        <span className="text-[12px] text-muted-foreground">
          {hoveredBucket
            ? hoveredBucket.sub_label
            : metric === "cost"
              ? "Total for period"
              : "Tokens for period"}
        </span>
        <span className="select-text font-mono text-[14px] font-semibold tabular-nums tracking-tight">
          {readoutValue}
        </span>
        <span className="select-text font-mono text-[11px] tabular-nums text-muted-foreground">
          {readoutBreakdown}
        </span>
      </div>

      <div
        className="mt-4 flex items-end gap-[3px]"
        style={{ height: CHART_HEIGHT_PX }}
        onMouseLeave={() => onHover(null)}
      >
        {buckets.map((bucket, index) => (
          <div
            key={bucket.start_ms}
            role="presentation"
            onMouseEnter={() => onHover(index)}
            title={`${bucket.sub_label} — ${formatMetric(
              order.reduce((sum, p) => sum + bucketValue(bucket, p, metric), 0),
              metric,
            )}`}
            className={cn(
              "flex h-full min-w-0 flex-1 flex-col justify-end gap-[2px] transition-opacity",
              hovered !== null && hovered !== index && "opacity-40",
            )}
          >
            {/* Reversed so the first lane (largest provider) stacks on
                top, matching the legend's reading order. */}
            {[...order].reverse().map((provider) => {
              const value = bucketValue(bucket, provider, metric);
              if (value <= 0) return null;
              return (
                <div
                  key={provider}
                  className={cn("w-full rounded-[2px]", seriesFill(provider))}
                  style={{
                    height: Math.max(2, (value / max) * (CHART_HEIGHT_PX - 8)),
                  }}
                />
              );
            })}
          </div>
        ))}
      </div>

      <div className="mt-3 flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
        <div className="flex flex-wrap items-center gap-x-4 gap-y-1 xl:gap-x-6">
          {providers.map((provider) => (
            <span
              key={provider.provider}
              className="inline-flex items-center gap-2 text-[11px]"
            >
              <span
                className={cn(
                  "h-2 w-2 shrink-0 rounded-[2px]",
                  seriesFill(provider.provider),
                )}
                aria-hidden
              />
              <span className="font-medium text-foreground">
                {seriesLabel(provider.provider)}
              </span>
              <span className="select-text font-mono tabular-nums text-muted-foreground">
                {metric === "cost"
                  ? formatMoney(provider.cost_usd)
                  : formatTokens(provider.tokens)}
              </span>
            </span>
          ))}
        </div>
        <span className="font-mono text-[10px] text-muted-foreground/80">
          {metric === "cost"
            ? "API/list-price equivalent · not an invoice"
            : "input + output + cache read + cache write"}
        </span>
      </div>
    </div>
  );
}

function HeroStat({
  label,
  value,
  note,
}: {
  label: string;
  value: string;
  note: string;
}) {
  return (
    <div className="flex flex-col gap-1.5">
      <span className="font-mono text-[10px] font-semibold uppercase tracking-[0.09em] text-muted-foreground/70">
        {label}
      </span>
      <span
        className={cn(
          "select-text font-mono text-[22px] font-semibold leading-none tabular-nums tracking-tight xl:text-[27px]",
          "text-foreground",
        )}
      >
        {value}
      </span>
      <span className="text-[11px] text-muted-foreground">{note}</span>
    </div>
  );
}

// ── token composition ──

/** A quiet, boxless strip of five figures, hairline-separated. It is a
 *  breakdown of the same period the cards above describe, so giving it a
 *  card of its own would imply a third independent subject. */
function CompositionRow({ composition }: { composition: UsageComposition }) {
  const c = composition;
  const cells: { label: string; value: string; note: string }[] = [
    {
      label: "Processed",
      value: formatTokens(c.processed_tokens),
      note: "tokens this period",
    },
    {
      label: "Cached input",
      value: formatTokens(c.cache_read_tokens),
      note: `${formatPercent(c.cache_read_share_of_input)} of input`,
    },
    {
      label: "Uncached input",
      value: formatTokens(c.input_tokens),
      note: `${formatTokens(c.cache_write_tokens)} cache writes`,
    },
    {
      label: "Output",
      value: formatTokens(c.output_tokens),
      // Only Codex and OpenCode split reasoning out; a Claude-only
      // period has nothing to say here and says nothing.
      note:
        c.reasoning_tokens > 0
          ? `includes ${formatTokens(c.reasoning_tokens)} reasoning`
          : "",
    },
    {
      label: "Cache savings",
      value: formatMoney(c.cache_savings_usd),
      note:
        c.cache_savings_multiplier != null
          ? `${c.cache_savings_multiplier.toFixed(1)}× vs uncached list price`
          : "vs uncached list price",
    },
  ];
  return (
    <div className="flex flex-wrap px-1">
      {cells.map((cell, i) => (
        <div
          key={cell.label}
          className={cn(
            "flex min-w-[150px] flex-1 flex-col gap-1 py-1 pr-5",
            i > 0 && "border-l border-border/60 pl-5",
          )}
        >
          <span className="font-mono text-[10px] font-semibold uppercase tracking-[0.09em] text-muted-foreground/70">
            {cell.label}
          </span>
          <span className="select-text font-mono text-[15px] tabular-nums">
            {cell.value}
          </span>
          <span className="text-[10px] text-muted-foreground">{cell.note}</span>
        </div>
      ))}
    </div>
  );
}

// ── flat model / day breakdown ──

type BreakdownView = "model" | "day";

const BREAKDOWN_OPTIONS: { value: BreakdownView; label: string }[] = [
  { value: "model", label: "Model" },
  { value: "day", label: "Day" },
];

/** "Where is my money going" — deliberately flat and cross-provider,
 *  next to the lanes card's "how is each provider behaving". */
function BreakdownCard({
  summary,
  view,
  onViewChange,
}: {
  summary: UsageSummary;
  view: BreakdownView;
  onViewChange: (view: BreakdownView) => void;
}) {
  const totalCost = summary.models.reduce((sum, m) => sum + m.cost_usd, 0);
  const totalTokens = summary.models.reduce((sum, m) => sum + m.tokens, 0);

  return (
    <div className="rounded-lg border border-border/60 bg-muted/30 p-4">
      <div className="mb-3 flex items-center justify-between gap-4">
        <p className="font-mono text-[10px] font-semibold uppercase tracking-[0.1em] text-muted-foreground/55">
          Breakdown
        </p>
        <SegmentedControl
          value={view}
          onChange={onViewChange}
          options={BREAKDOWN_OPTIONS}
          ariaLabel="Breakdown grouping"
          size="sm"
        />
      </div>

      <div className="flex flex-col gap-4 lg:flex-row lg:items-start">
        <div className="min-w-0 flex-1">
          {view === "model" ? (
            <ModelRows
              models={summary.models}
              totalCost={totalCost}
              totalTokens={totalTokens}
            />
          ) : (
            <DayRows summary={summary} />
          )}
        </div>
        <CostConfidenceBlock confidence={summary.confidence} />
      </div>
    </div>
  );
}

function ModelRows({
  models,
  totalCost,
  totalTokens,
}: {
  models: FlatModelUsage[];
  totalCost: number;
  totalTokens: number;
}) {
  if (models.length === 0) {
    return (
      <p className="py-4 text-[12px] text-muted-foreground">No models yet.</p>
    );
  }
  return (
    <div className="flex flex-col">
      {models.map((m, i) => (
        <div
          key={`${m.provider}-${m.model}`}
          className={cn(
            "flex items-center gap-3 py-1.5",
            i > 0 && "border-t border-border/30",
          )}
        >
          {isKnownProvider(m.provider) ? (
            <ProviderLogo provider={m.provider} className="h-3.5 w-3.5" />
          ) : (
            <span className="h-3.5 w-3.5" aria-hidden />
          )}
          <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-muted-foreground">
            {m.model}
          </span>
          <span className="w-[70px] shrink-0 select-text text-right font-mono text-[12px] tabular-nums">
            {/* An unpriced model has no cost to show — an em-dash is
                honest where "$0.00" would read as free. */}
            {m.priced ? formatMoney(m.cost_usd) : "—"}
          </span>
          <span className="w-[64px] shrink-0 text-right font-mono text-[10px] tabular-nums text-muted-foreground">
            {m.priced
              ? formatPercent(totalCost > 0 ? m.cost_usd / totalCost : 0)
              : `${formatPercent(totalTokens > 0 ? m.tokens / totalTokens : 0)} tok`}
          </span>
          <span className="w-[64px] shrink-0 select-text text-right font-mono text-[11px] tabular-nums text-muted-foreground">
            {formatTokens(m.tokens)}
          </span>
        </div>
      ))}
    </div>
  );
}

function DayRows({ summary }: { summary: UsageSummary }) {
  // Derived from the buckets the chart already uses, so the two can
  // never disagree. Newest first — the opposite of the chart's axis,
  // because a table is read from the top.
  const rows = summary.buckets
    .map((bucket) => {
      const providers = Object.values(bucket.providers);
      return {
        key: bucket.start_ms,
        label: bucket.sub_label,
        cost: providers.reduce((sum, p) => sum + p.cost_usd, 0),
        tokens: providers.reduce((sum, p) => sum + p.tokens, 0),
      };
    })
    .filter((r) => r.tokens > 0)
    .reverse();
  const totalCost = rows.reduce((sum, r) => sum + r.cost, 0);

  if (rows.length === 0) {
    return (
      <p className="py-4 text-[12px] text-muted-foreground">
        No activity yet.
      </p>
    );
  }
  return (
    <div className="flex flex-col">
      {rows.map((r, i) => (
        <div
          key={r.key}
          className={cn(
            "flex items-center gap-3 py-1.5",
            i > 0 && "border-t border-border/30",
          )}
        >
          <span className="min-w-0 flex-1 truncate text-[11px] text-muted-foreground">
            {r.label}
          </span>
          <span className="w-[70px] shrink-0 select-text text-right font-mono text-[12px] tabular-nums">
            {formatMoney(r.cost)}
          </span>
          <span className="w-[64px] shrink-0 text-right font-mono text-[10px] tabular-nums text-muted-foreground">
            {formatPercent(totalCost > 0 ? r.cost / totalCost : 0)}
          </span>
          <span className="w-[64px] shrink-0 select-text text-right font-mono text-[11px] tabular-nums text-muted-foreground">
            {formatTokens(r.tokens)}
          </span>
        </div>
      ))}
    </div>
  );
}

/** Makes the estimated nature of most cost figures visible rather than
 *  implicit. Rows render even at 0% so the block keeps a stable shape. */
function CostConfidenceBlock({ confidence }: { confidence: CostConfidence }) {
  const rows: { label: string; value: string }[] = [
    {
      label: "Provider reported",
      value: formatPercent(confidence.provider_reported_share),
    },
    { label: "Model priced", value: formatPercent(confidence.table_priced_share) },
    { label: "Unpriced", value: `${formatPercent(confidence.unpriced_token_share)} tok` },
    { label: "Cache savings", value: formatMoney(confidence.cache_savings_usd) },
  ];
  return (
    <div className="shrink-0 lg:w-[200px] lg:border-l lg:border-border/60 lg:pl-4">
      <p className="mb-2 font-mono text-[10px] font-semibold uppercase tracking-[0.1em] text-muted-foreground/55">
        Cost confidence
      </p>
      <div className="flex flex-col gap-1">
        {rows.map((r) => (
          <div key={r.label} className="flex items-baseline justify-between gap-3">
            <span className="text-[11px] text-muted-foreground">{r.label}</span>
            <span className="select-text font-mono text-[11px] tabular-nums">
              {r.value}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}

// ── provider-history footer ──

function ProviderHistoryFooter({
  busy,
  sessionCount,
}: {
  busy: boolean;
  sessionCount: number;
}) {
  return (
    <div className="flex items-baseline gap-4 px-1 pt-1">
      <p className="min-w-0 flex-1 text-[11px] leading-relaxed text-muted-foreground">
        <span className="font-medium text-foreground">
          Includes {sessionCount.toLocaleString()} provider session
          {sessionCount === 1 ? "" : "s"}
        </span>{" "}
        from this machine&apos;s Claude Code, Codex, and OpenCode histories,
        regardless of which app launched them. Sources include{" "}
        <span className="font-mono text-[10px]">~/.claude/projects</span>,{" "}
        <span className="font-mono text-[10px]">~/.codex/sessions</span>, and
        OpenCode&apos;s local data directory. This machine only.
      </p>
      {busy && (
        <span className="shrink-0 font-mono text-[10px] text-muted-foreground">
          scanning…
        </span>
      )}
    </div>
  );
}

// ── provider lanes ──

const SPARK_HEIGHT_PX = 30;

function LanesCard({
  summary,
  expanded,
  onToggle,
}: {
  summary: UsageSummary;
  expanded: string | null;
  onToggle: (provider: string) => void;
}) {
  return (
    <div className="rounded-lg border border-border/60 bg-muted/30 px-4">
      {summary.providers.map((provider, index) => (
        <ProviderLane
          key={provider.provider}
          provider={provider}
          quota={summary.quota?.[provider.provider]}
          buckets={summary.buckets}
          open={expanded === provider.provider}
          onToggle={() => onToggle(provider.provider)}
          divided={index > 0}
        />
      ))}
    </div>
  );
}

function ProviderLane({
  provider,
  quota,
  buckets,
  open,
  onToggle,
  divided,
}: {
  provider: UsageProvider;
  /** Live plan quota, when the provider reports any. Absent → the lane
   *  renders exactly as it did before meters existed. */
  quota?: ProviderQuota;
  buckets: UsageBucket[];
  open: boolean;
  onToggle: () => void;
  divided: boolean;
}) {
  const bars = quota ? meterWindows(quota.windows) : [];
  const note = quota ? meterNote(quota) : "";
  const laneMax = useMemo(
    () =>
      Math.max(
        ...buckets.map((b) => b.providers[provider.provider]?.tokens ?? 0),
        0,
      ) || 1,
    [buckets, provider.provider],
  );

  return (
    <div className={cn(divided && "border-t border-border/40")}>
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={open}
        className="flex w-full items-center gap-4 py-3.5 text-left transition-colors hover:bg-accent/20"
      >
        {/* Widens with the column so provider plan labels do not truncate;
            the model rows below indent to match. */}
        <span className="flex w-[150px] shrink-0 items-center gap-2.5 xl:w-[200px]">
          {isKnownProvider(provider.provider) ? (
            <ProviderLogo provider={provider.provider} className="h-[18px] w-[18px]" />
          ) : (
            <span className="h-[18px] w-[18px]" aria-hidden />
          )}
          <span className="flex min-w-0 flex-col gap-0.5">
            <span className="truncate text-[13px] font-medium">
              {seriesLabel(provider.provider)}
            </span>
            <span className="truncate text-[10px] text-muted-foreground">
              {quota?.plan_label ?? "Provider history"}
            </span>
          </span>
        </span>

        {/* Quota meters when the provider reports a plan, sparkline
            otherwise. A provider with no quota data (OpenCode, or one
            that has not run this session) renders exactly as before —
            an empty meter would imply a limit that does not exist. */}
        {bars.length > 0 ? (
          <span className="flex w-[168px] shrink-0 flex-col gap-1.5">
            {bars.map((w) => (
              <span key={w.kind} className="flex items-center gap-2">
                <span className="w-8 shrink-0 text-[10px] text-muted-foreground">
                  {WINDOW_LABEL[w.kind]}
                </span>
                <span className="h-1 flex-1 overflow-hidden rounded-full bg-muted-foreground/20">
                  <span
                    className={cn("block h-full rounded-full", meterTone(w.used_pct))}
                    style={{ width: `${Math.min(100, Math.max(0, w.used_pct))}%` }}
                  />
                </span>
                <span className="w-8 shrink-0 select-text text-right font-mono text-[10px] tabular-nums text-muted-foreground">
                  {Math.round(w.used_pct)}%
                </span>
              </span>
            ))}
            {note && (
              <span className="truncate text-[10px] text-muted-foreground/80">
                {note}
              </span>
            )}
          </span>
        ) : null}
        <span
          className="flex min-w-0 flex-1 items-end gap-[2px]"
          style={{ height: SPARK_HEIGHT_PX }}
          aria-hidden
        >
          {buckets.map((bucket) => {
            const tokens = bucket.providers[provider.provider]?.tokens ?? 0;
            return (
              <span
                key={bucket.start_ms}
                className={cn(
                  "min-w-0 flex-1 rounded-[1px]",
                  seriesFill(provider.provider),
                )}
                style={{
                  height: Math.max(2, (tokens / laneMax) * SPARK_HEIGHT_PX),
                }}
              />
            );
          })}
        </span>

        <span className="flex w-[76px] shrink-0 flex-col gap-0.5 text-right">
          <span className="select-text font-mono text-[13px] tabular-nums">
            {formatTokens(provider.tokens)}
          </span>
          <span className="text-[10px] text-muted-foreground">tokens</span>
        </span>

        <span className="flex w-[96px] shrink-0 flex-col gap-0.5 text-right">
          <span
            className="select-text font-mono text-[15px] font-semibold tabular-nums tracking-tight"
          >
            {formatMoney(provider.cost_usd)}
          </span>
          <span className="text-[10px] text-muted-foreground">API equivalent</span>
        </span>

        <span className="shrink-0 text-muted-foreground" aria-hidden>
          {open ? (
            <ChevronUp className="h-3.5 w-3.5" />
          ) : (
            <ChevronDown className="h-3.5 w-3.5" />
          )}
        </span>
      </button>

      {open && (
        <div className="pb-3 pl-[150px] xl:pl-[200px]">
          {provider.models.map((model) => {
            const share =
              provider.tokens > 0 ? model.tokens / provider.tokens : 0;
            return (
              <div
                key={model.model}
                className="flex items-center gap-4 border-t border-border/30 py-1.5"
              >
                <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-muted-foreground">
                  {model.model}
                </span>
                <span className="w-[110px] shrink-0 text-[10px] text-muted-foreground">
                  {formatPercent(share)} of tokens
                  {model.subagent_tokens > 0 && " · subagents"}
                </span>
                <span className="w-[76px] shrink-0 select-text text-right font-mono text-[11px] tabular-nums text-muted-foreground">
                  {formatTokens(model.tokens)}
                </span>
                <span className="w-[96px] shrink-0 select-text text-right font-mono text-[11px] tabular-nums text-muted-foreground">
                  {formatMoney(model.cost_usd)}
                </span>
                <span className="w-3.5 shrink-0" aria-hidden />
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
