import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  ChevronDown,
  ChevronUp,
  Loader2,
  RefreshCw,
  SlidersHorizontal,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { ProviderLogo } from "@/components/chat/provider-logo";
import { SegmentedControl } from "./settings-primitives";
import { UsageAreaChart, UsageSparkline } from "./usage-area-chart";
import {
  usageExportCsv,
  usagePriceOverrides,
  usageRefreshQuota,
  usageScanProviderHistory,
  usageSummary,
} from "@/tauri/commands";
import type {
  CostConfidence,
  FlatModelUsage,
  PlanUsageWindow,
  PriceOverride,
  QuotaProbeStatus,
  UsageComposition,
  PlanWindowKind,
  ProviderQuota,
  UsageBucket,
  UsagePeriod,
  UsageProvider,
  UsageSummary,
} from "@/tauri/commands";
import { toast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { Eyebrow } from "@/components/ui/eyebrow";
import {
  formatMoney,
  formatPercent,
  formatTokens,
  isKnownProvider,
  own,
  seriesColor,
  seriesFill,
  seriesLabel,
} from "./usage-format";
import { UsageLimitsView, formatCheckedAgo } from "./usage-limits";
import {
  ModelPricesDialog,
  PriceEditorDialog,
  UsageModelDialog,
} from "./usage-model-dialog";
import {
  readUsagePreferences,
  saveUsagePreferences,
  type UsageBreakdown,
  type UsageMetric,
  type UsagePreferences,
  type UsageView,
} from "./usage-preferences";
import { UsageShareBar, costTypeSegments, tokenTypeSegments } from "./usage-share-bar";

export { formatMoney, formatTokens } from "./usage-format";

/** How often to re-poll while the page is open. The ledger only grows
 *  when an agent is mid-turn, so this is about keeping an open settings
 *  tab honest, not about being live. */
const POLL_MS = 30_000;

/** Entering Limits re-reads the providers when the last read is older
 *  than this; each read briefly spawns the Claude and Codex CLIs. */
const QUOTA_REFRESH_AFTER_MS = 60_000;

/** Countdown cadence on the Limits view. Minutes are the finest unit it
 *  shows, so a faster tick would repaint for nothing. */
const LIMITS_TICK_MS = 30_000;

const VIEW_OPTIONS: { value: UsageView; label: string }[] = [
  { value: "activity", label: "Usage" },
  { value: "limits", label: "Limits" },
];

const PERIOD_OPTIONS: { value: UsagePeriod; label: string }[] = [
  { value: "today", label: "Today" },
  { value: "7d", label: "7 days" },
  { value: "30d", label: "30 days" },
  { value: "90d", label: "90 days" },
];

type Metric = UsageMetric;

const METRIC_OPTIONS: { value: Metric; label: string }[] = [
  { value: "cost", label: "Est. cost" },
  { value: "tokens", label: "Tokens" },
];

/** Per provider, whichever reading is newer: the summary's snapshot of
 *  the session-fed store, or the last direct read. */
function newestQuota(
  ...sources: (Record<string, ProviderQuota> | undefined)[]
): Record<string, ProviderQuota> {
  const merged: Record<string, ProviderQuota> = {};
  for (const source of sources) {
    for (const [provider, quota] of Object.entries(source ?? {})) {
      const current = merged[provider];
      if (!current || quota.received_at_ms >= current.received_at_ms) {
        merged[provider] = quota;
      }
    }
  }
  return merged;
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

/**
 * "Usage" settings section — token and cost accounting per provider,
 * model, and session, read from the local `agent_usage_ledger`.
 *
 * Cost is always an API/list-price equivalent. Subscription quota is shown
 * separately when a provider reports it; the page never guesses what was
 * actually billed.
 */
export function UsageSection() {
  const [preferences, setPreferences] = useState<UsagePreferences>(readUsagePreferences);
  const { view, period, metric, breakdown } = preferences;
  const updatePreferences = useCallback((patch: Partial<UsagePreferences>) => {
    setPreferences((current) => {
      const next = { ...current, ...patch };
      saveUsagePreferences(next);
      return next;
    });
  }, []);
  const setPeriod = (value: UsagePeriod) => updatePreferences({ period: value });
  const setMetric = (value: Metric) => updatePreferences({ metric: value });
  const setBreakdown = (value: UsageBreakdown) => updatePreferences({ breakdown: value });

  const [summary, setSummary] = useState<UsageSummary | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [hovered, setHovered] = useState<number | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [exporting, setExporting] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [scanning, setScanning] = useState(false);
  /// Whether the open-time provider-history scan has settled.
  const [scanned, setScanned] = useState(false);

  // Plan limits read directly from the providers (see `usage_quota.rs`).
  const [directQuota, setDirectQuota] = useState<Record<string, ProviderQuota>>();
  const [quotaStatuses, setQuotaStatuses] = useState<QuotaProbeStatus[]>([]);
  /// The refresh command itself failed, so no provider was read at all.
  const [quotaError, setQuotaError] = useState<string | null>(null);
  const [quotaLoading, setQuotaLoading] = useState(true);
  const lastQuotaRefresh = useRef(0);
  const [now, setNow] = useState(() => Date.now());

  const [overrides, setOverrides] = useState<Record<string, PriceOverride>>({});
  const [selectedModelKey, setSelectedModelKey] = useState<string | null>(null);
  const [priceEditor, setPriceEditor] = useState<{
    model: string | null;
    rates: PriceOverride | null;
  } | null>(null);
  const [pricesOpen, setPricesOpen] = useState(false);

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

  const refreshQuota = useCallback(async () => {
    lastQuotaRefresh.current = Date.now();
    setQuotaLoading(true);
    try {
      const report = await usageRefreshQuota();
      setQuotaError(null);
      setDirectQuota(report.quota);
      // A call that joined a read already in flight reports no statuses;
      // keep the ones that read produced.
      if (report.statuses.length > 0) setQuotaStatuses(report.statuses);
    } catch (err) {
      setQuotaError(String(err));
    } finally {
      setQuotaLoading(false);
      setNow(Date.now());
    }
  }, []);

  // A scan on page open keeps provider history current. The scan is
  // incremental, so unchanged sources are cheap. Limits are read alongside,
  // so the lane meters and the Limits view do not wait for a session to
  // report them.
  //
  // Deliberately mount-only: a period change cannot affect source data.
  useEffect(() => {
    void scan().finally(() => setScanned(true));
    void refreshQuota();
    void usagePriceOverrides()
      .then(setOverrides)
      .catch(() => {});
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

  // Entering Limits with an old reading re-reads it; while Limits is open
  // the countdowns advance on their own.
  useEffect(() => {
    if (view !== "limits") return;
    if (Date.now() - lastQuotaRefresh.current > QUOTA_REFRESH_AFTER_MS) {
      void refreshQuota();
    }
    setNow(Date.now());
    const id = window.setInterval(() => setNow(Date.now()), LIMITS_TICK_MS);
    return () => window.clearInterval(id);
  }, [view, refreshQuota]);

  // A hovered bucket index is only meaningful for the period it was
  // taken in — a stale index would read out the wrong bar.
  useEffect(() => {
    setHovered(null);
  }, [period]);

  const quota = useMemo(
    () => newestQuota(summary?.quota, directQuota),
    [summary?.quota, directQuota],
  );
  const selectedModel =
    selectedModelKey === null
      ? undefined
      : summary?.models.find((m) => `${m.provider}:${m.model}` === selectedModelKey);

  const handleRefresh = () => {
    if (view === "limits") {
      void refreshQuota();
      return;
    }
    void scanThenRefresh();
    void refreshQuota();
  };

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

  const onPricesChanged = (next: Record<string, PriceOverride>) => {
    setOverrides(next);
    void refresh();
  };

  const busy = view === "limits" ? quotaLoading : refreshing || scanning;
  const newestReading = Math.max(
    0,
    ...Object.values(quota)
      .filter((q) => q.windows.length > 0)
      .map((q) => q.received_at_ms),
  );

  return (
    <div>
      <div className="mb-6 flex items-end justify-between gap-4">
        <div className="min-w-0">
          <h2 className="text-body-lg font-semibold tracking-tight">Usage</h2>
          <p className="mt-1 text-body-sm text-muted-foreground">
            {view === "limits"
              ? quotaLoading
                ? "Plan limits · reading…"
                : newestReading > 0
                  ? `Plan limits · checked ${formatCheckedAgo(newestReading, now)}`
                  : "Plan limits"
              : `${summary ? rangeLabel(summary) : "Loading…"} · ${
                  refreshing ? "refreshing…" : "live"
                }`}
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <SegmentedControl
            value={view}
            onChange={(value) => updatePreferences({ view: value })}
            options={VIEW_OPTIONS}
            ariaLabel="Usage view"
          />
          {/* The period does not apply to Limits; it stays in place but
              disabled, so switching views does not shift the controls. */}
          <SegmentedControl
            value={period}
            onChange={setPeriod}
            options={PERIOD_OPTIONS}
            ariaLabel="Usage period"
            disabled={view === "limits"}
          />
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={view === "limits" ? "Refresh limits" : "Refresh usage"}
            onClick={handleRefresh}
            disabled={busy}
          >
            <RefreshCw
              className={cn("size-3.5", busy && "animate-spin")}
              aria-hidden
            />
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={handleExport}
            disabled={exporting || !summary || view === "limits"}
          >
            Export CSV
          </Button>
        </div>
      </div>

      {view === "limits" ? (
        <UsageLimitsView
          quota={quota}
          statuses={quotaStatuses}
          error={quotaError}
          now={now}
          loading={quotaLoading}
        />
      ) : (
        <>
          {error && (
            <p className="mb-4 rounded-md bg-destructive/10 px-3 py-2 text-label text-destructive">
              Failed to load usage: {error}
            </p>
          )}

          {summary === null ? (
            !error && (
              <div className="flex items-center gap-2 py-6 text-body text-muted-foreground">
                <Loader2 className="size-4 animate-spin" aria-hidden />
                Loading usage…
              </div>
            )
          ) : summary.totals.total_tokens === 0 ? (
            <div className="rounded-lg border border-border/60 bg-muted/30 px-4 py-10 text-center text-body text-muted-foreground">
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
              <TypeSplitCard summary={summary} />
              <LanesCard
                summary={summary}
                quota={quota}
                expanded={expanded}
                onToggle={(provider) =>
                  setExpanded((current) => (current === provider ? null : provider))
                }
              />
              <BreakdownCard
                summary={summary}
                view={breakdown}
                onViewChange={setBreakdown}
                onSelectModel={(model) =>
                  setSelectedModelKey(`${model.provider}:${model.model}`)
                }
                onOpenPrices={() => setPricesOpen(true)}
              />
              <ProviderHistoryFooter busy={scanning} sessionCount={summary.totals.session_count} />
            </div>
          )}
        </>
      )}

      {selectedModel && summary && view === "activity" && (
        <UsageModelDialog
          model={selectedModel}
          buckets={summary.buckets}
          totalCost={summary.totals.estimated_cost_usd}
          metric={metric}
          onClose={() => setSelectedModelKey(null)}
          onEditPrice={(model) => {
            setSelectedModelKey(null);
            setPriceEditor({
              model: model.model,
              rates: own(overrides, model.model) ?? model.rates,
            });
          }}
        />
      )}
      {pricesOpen && (
        <ModelPricesDialog
          overrides={overrides}
          onClose={() => setPricesOpen(false)}
          onChanged={onPricesChanged}
          onAdd={() => {
            setPricesOpen(false);
            setPriceEditor({ model: null, rates: null });
          }}
          onEdit={(model) => {
            setPricesOpen(false);
            setPriceEditor({ model, rates: own(overrides, model) ?? null });
          }}
        />
      )}
      {priceEditor && (
        <PriceEditorDialog
          model={priceEditor.model}
          initialRates={priceEditor.rates}
          hasOverride={
            priceEditor.model !== null && own(overrides, priceEditor.model) !== undefined
          }
          onClose={() => setPriceEditor(null)}
          onSaved={onPricesChanged}
        />
      )}
    </div>
  );
}

/** First bucket to last, in the backend's own words. "Today" is a
 *  trailing 24-hour window rather than a calendar day, so it reads
 *  "Yesterday 13:00 – Today 12:00" — naming it "Today, 00:00 – now" would
 *  claim a range the chart does not show. */
function rangeLabel(summary: UsageSummary): string {
  const first = summary.buckets[0];
  const last = summary.buckets[summary.buckets.length - 1];
  if (!first || !last) return "";
  return `${first.sub_label} – ${last.sub_label}`;
}

/** The value one bucket contributes for one provider, under the active
 *  metric. Shared by the chart and its tooltip so a hovered point and its
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
const CHART_HEIGHT_PX = 200;

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

  const series = useMemo(
    () =>
      order.map((p) => ({
        key: p,
        label: seriesLabel(p),
        ...seriesColor(p),
        values: buckets.map((bucket) => bucketValue(bucket, p, metric)),
      })),
    [buckets, order, metric],
  );
  const points = useMemo(
    () => buckets.map((b) => ({ label: b.label, subLabel: b.sub_label })),
    [buckets],
  );

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

      <UsageAreaChart
        className="mt-5"
        series={series}
        points={points}
        height={CHART_HEIGHT_PX}
        hovered={hovered}
        onHover={onHover}
        formatValue={(v) => formatMetric(v, metric)}
        ariaLabel={
          metric === "cost"
            ? "Estimated cost per bucket by provider"
            : "Tokens per bucket by provider"
        }
      />

      <div className="mt-3 flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
        <div className="flex flex-wrap items-center gap-x-4 gap-y-1 xl:gap-x-6">
          {providers.map((provider) => (
            <span
              key={provider.provider}
              className="inline-flex items-center gap-2 text-label"
            >
              <span
                className={cn(
                  "size-2 shrink-0 rounded-sm",
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
        <span className="font-mono text-caption text-muted-foreground/80">
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
      <Eyebrow>
        {label}
      </Eyebrow>
      <span
        className={cn(
          "select-text font-mono text-[1.375rem] font-semibold leading-none tabular-nums tracking-tight xl:text-[1.6875rem]",
          "text-foreground",
        )}
      >
        {value}
      </span>
      <span className="text-label text-muted-foreground">{note}</span>
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
          <Eyebrow>
            {cell.label}
          </Eyebrow>
          <span className="select-text font-mono text-body-lg tabular-nums">
            {cell.value}
          </span>
          <span className="text-caption text-muted-foreground">{cell.note}</span>
        </div>
      ))}
    </div>
  );
}

// ── cost and tokens by type ──

/** Where the money went (input, output, cache reads and writes) beside
 *  where the tokens went. They differ sharply: cache reads dominate the
 *  tokens and barely register in the cost. */
function TypeSplitCard({ summary }: { summary: UsageSummary }) {
  const tokens = {
    input_tokens: summary.composition.input_tokens,
    cache_read_tokens: summary.composition.cache_read_tokens,
    cache_write_tokens: summary.composition.cache_write_tokens,
    output_tokens: summary.composition.output_tokens,
  };
  return (
    <div className="grid gap-x-10 gap-y-5 rounded-lg border border-border/60 bg-muted/30 p-4 lg:grid-cols-2">
      <UsageShareBar
        label="Cost by type"
        segments={costTypeSegments(summary.category_cost)}
        format={formatMoney}
      />
      <UsageShareBar
        label="Tokens by type"
        segments={tokenTypeSegments(tokens)}
        format={formatTokens}
      />
    </div>
  );
}

// ── flat model / day breakdown ──

type BreakdownView = UsageBreakdown;

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
  onSelectModel,
  onOpenPrices,
}: {
  summary: UsageSummary;
  view: BreakdownView;
  onViewChange: (view: BreakdownView) => void;
  onSelectModel: (model: FlatModelUsage) => void;
  onOpenPrices: () => void;
}) {
  const totalCost = summary.models.reduce((sum, m) => sum + m.cost_usd, 0);
  const totalTokens = summary.models.reduce((sum, m) => sum + m.tokens, 0);

  return (
    <div className="rounded-lg border border-border/60 bg-muted/30 p-4">
      <div className="mb-3 flex items-center justify-between gap-4">
        <Eyebrow>
          Breakdown
        </Eyebrow>
        <div className="flex items-center gap-2">
          <Button variant="ghost" size="sm" onClick={onOpenPrices}>
            <SlidersHorizontal className="size-3.5" aria-hidden />
            Model prices
          </Button>
          <SegmentedControl
            value={view}
            onChange={onViewChange}
            options={BREAKDOWN_OPTIONS}
            ariaLabel="Breakdown grouping"
            size="sm"
          />
        </div>
      </div>

      <div className="flex flex-col gap-4 lg:flex-row lg:items-start">
        <div className="min-w-0 flex-1">
          {view === "model" ? (
            <ModelRows
              models={summary.models}
              totalCost={totalCost}
              totalTokens={totalTokens}
              onSelect={onSelectModel}
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
  onSelect,
}: {
  models: FlatModelUsage[];
  totalCost: number;
  totalTokens: number;
  onSelect: (model: FlatModelUsage) => void;
}) {
  if (models.length === 0) {
    return (
      <p className="py-4 text-body-sm text-muted-foreground">No models yet.</p>
    );
  }
  return (
    <div className="flex flex-col">
      {models.map((m, i) => (
        <button
          type="button"
          key={`${m.provider}-${m.model}`}
          onClick={() => onSelect(m)}
          aria-label={`Open ${m.model} details`}
          className={cn(
            "-mx-2 flex items-center gap-3 rounded-md px-2 py-1.5 text-left transition-colors duration-150 hover:bg-accent/30 focus-visible:bg-accent/30 focus-visible:outline-none",
            i > 0 && "border-t border-border/30",
          )}
        >
          {isKnownProvider(m.provider) ? (
            <ProviderLogo provider={m.provider} className="size-3.5" />
          ) : (
            <span className="size-3.5" aria-hidden />
          )}
          <span className="min-w-0 flex-1 truncate font-mono text-label text-muted-foreground">
            {m.model}
            {m.price_overridden && (
              <span className="ml-2 font-sans text-caption text-muted-foreground/80">
                your price
              </span>
            )}
          </span>
          <span className="w-[78px] shrink-0 select-text text-right font-mono text-body-sm tabular-nums">
            {/* An unpriced model has no cost to show — an em-dash is
                honest where "$0.00" would read as free. */}
            {m.priced ? formatMoney(m.cost_usd) : "—"}
          </span>
          <span className="w-[64px] shrink-0 text-right font-mono text-caption tabular-nums text-muted-foreground">
            {m.priced
              ? formatPercent(totalCost > 0 ? m.cost_usd / totalCost : 0)
              : `${formatPercent(totalTokens > 0 ? m.tokens / totalTokens : 0)} tok`}
          </span>
          <span className="w-[64px] shrink-0 select-text text-right font-mono text-label tabular-nums text-muted-foreground">
            {formatTokens(m.tokens)}
          </span>
        </button>
      ))}
    </div>
  );
}

function DayRows({ summary }: { summary: UsageSummary }) {
  // Derived from the buckets the chart already uses, so the two can
  // never disagree. Newest first — the opposite of the chart's axis,
  // because a table is read from the top. One cost column per provider,
  // in lane order, so a spike can be traced to whoever caused it.
  const order = summary.providers.map((p) => p.provider);
  const rows = summary.buckets
    .map((bucket) => {
      const providers = Object.values(bucket.providers);
      return {
        key: bucket.start_ms,
        label: bucket.sub_label,
        byProvider: bucket.providers,
        cost: providers.reduce((sum, p) => sum + p.cost_usd, 0),
        tokens: providers.reduce((sum, p) => sum + p.tokens, 0),
      };
    })
    .filter((r) => r.tokens > 0)
    .reverse();

  if (rows.length === 0) {
    return (
      <p className="py-4 text-body-sm text-muted-foreground">
        No activity yet.
      </p>
    );
  }
  const columns = `minmax(5.5rem,1fr) repeat(${order.length}, minmax(4.5rem,5.5rem)) 5.5rem 4.5rem`;
  return (
    <div className="flex flex-col overflow-x-auto">
      <div
        className="grid items-center gap-x-3 pb-1.5 text-caption text-muted-foreground"
        style={{ gridTemplateColumns: columns }}
      >
        <span>{summary.period === "today" ? "Hour" : "Day"}</span>
        {order.map((provider) => (
          <span key={provider} className="flex items-center justify-end gap-1.5 truncate">
            <span
              className={cn("size-1.5 shrink-0 rounded-sm", seriesFill(provider))}
              aria-hidden
            />
            {seriesLabel(provider)}
          </span>
        ))}
        <span className="text-right">Total</span>
        <span className="text-right">Tokens</span>
      </div>
      {rows.map((r) => (
        <div
          key={r.key}
          className="grid items-center gap-x-3 border-t border-border/30 py-1.5"
          style={{ gridTemplateColumns: columns }}
        >
          <span className="min-w-0 truncate text-label text-muted-foreground">
            {r.label}
          </span>
          {order.map((provider) => (
            <span
              key={provider}
              className="select-text text-right font-mono text-label tabular-nums text-muted-foreground"
            >
              {r.byProvider[provider] ? formatMoney(r.byProvider[provider].cost_usd) : "—"}
            </span>
          ))}
          <span className="select-text text-right font-mono text-body-sm tabular-nums">
            {formatMoney(r.cost)}
          </span>
          <span className="select-text text-right font-mono text-label tabular-nums text-muted-foreground">
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
    {
      label: "Your prices",
      value: formatPercent(confidence.override_priced_share ?? 0),
    },
    { label: "Unpriced", value: `${formatPercent(confidence.unpriced_token_share)} tok` },
    { label: "Cache savings", value: formatMoney(confidence.cache_savings_usd) },
  ];
  return (
    <div className="shrink-0 lg:w-[200px] lg:border-l lg:border-border/60 lg:pl-4">
      <Eyebrow className="mb-2">
        Cost confidence
      </Eyebrow>
      <div className="flex flex-col gap-1">
        {rows.map((r) => (
          <div key={r.label} className="flex items-baseline justify-between gap-3">
            <span className="text-label text-muted-foreground">{r.label}</span>
            <span className="select-text font-mono text-label tabular-nums">
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
      <p className="min-w-0 flex-1 text-label leading-relaxed text-muted-foreground">
        <span className="font-medium text-foreground">
          Includes {sessionCount.toLocaleString()} provider session
          {sessionCount === 1 ? "" : "s"}
        </span>{" "}
        from this machine&apos;s Claude Code, Codex, and OpenCode histories,
        regardless of which app launched them. Sources include{" "}
        <span className="font-mono text-caption">~/.claude/projects</span>,{" "}
        <span className="font-mono text-caption">~/.codex/sessions</span>, and
        OpenCode&apos;s local data directory. This machine only.
      </p>
      {busy && (
        <span className="shrink-0 font-mono text-caption text-muted-foreground">
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
  quota,
  expanded,
  onToggle,
}: {
  summary: UsageSummary;
  quota: Record<string, ProviderQuota>;
  expanded: string | null;
  onToggle: (provider: string) => void;
}) {
  return (
    <div className="rounded-lg border border-border/60 bg-muted/30 px-4">
      {summary.providers.map((provider, index) => (
        <ProviderLane
          key={provider.provider}
          provider={provider}
          quota={quota[provider.provider]}
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
  const laneValues = useMemo(
    () => buckets.map((b) => b.providers[provider.provider]?.tokens ?? 0),
    [buckets, provider.provider],
  );

  return (
    <div className={cn(divided && "border-t border-border/40")}>
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={open}
        className="flex w-full items-center gap-4 py-3.5 text-left transition-colors duration-150 hover:bg-accent/20"
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
            <span className="truncate text-body font-medium">
              {seriesLabel(provider.provider)}
            </span>
            <span className="truncate text-caption text-muted-foreground">
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
                <span className="w-9 shrink-0 text-caption text-muted-foreground">
                  {WINDOW_LABEL[w.kind]}
                </span>
                <span className="h-1 flex-1 overflow-hidden rounded-full bg-muted-foreground/20">
                  <span
                    className={cn("block h-full rounded-full", meterTone(w.used_pct))}
                    style={{ width: `${Math.min(100, Math.max(0, w.used_pct))}%` }}
                  />
                </span>
                <span className="w-8 shrink-0 select-text text-right font-mono text-caption tabular-nums text-muted-foreground">
                  {Math.round(w.used_pct)}%
                </span>
              </span>
            ))}
            {note && (
              <span className="truncate text-caption text-muted-foreground/80">
                {note}
              </span>
            )}
          </span>
        ) : null}
        <span className="min-w-0 flex-1 self-center">
          <UsageSparkline
            values={laneValues}
            height={SPARK_HEIGHT_PX}
            {...seriesColor(provider.provider)}
          />
        </span>

        <span className="flex w-[76px] shrink-0 flex-col gap-0.5 text-right">
          <span className="select-text font-mono text-body tabular-nums">
            {formatTokens(provider.tokens)}
          </span>
          <span className="text-caption text-muted-foreground">tokens</span>
        </span>

        <span className="flex w-[96px] shrink-0 flex-col gap-0.5 text-right">
          <span
            className="select-text font-mono text-body-lg font-semibold tabular-nums tracking-tight"
          >
            {formatMoney(provider.cost_usd)}
          </span>
          <span className="text-caption text-muted-foreground">API equivalent</span>
        </span>

        <span className="shrink-0 text-muted-foreground" aria-hidden>
          {open ? (
            <ChevronUp className="size-3.5" />
          ) : (
            <ChevronDown className="size-3.5" />
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
                <span className="min-w-0 flex-1 truncate font-mono text-label text-muted-foreground">
                  {model.model}
                </span>
                <span className="w-[110px] shrink-0 text-caption text-muted-foreground">
                  {formatPercent(share)} of tokens
                  {model.subagent_tokens > 0 && " · subagents"}
                </span>
                <span className="w-[76px] shrink-0 select-text text-right font-mono text-label tabular-nums text-muted-foreground">
                  {formatTokens(model.tokens)}
                </span>
                <span className="w-[96px] shrink-0 select-text text-right font-mono text-label tabular-nums text-muted-foreground">
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
