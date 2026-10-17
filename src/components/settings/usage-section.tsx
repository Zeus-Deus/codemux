import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";

import {
  ChevronDown,
  ChevronRight,
  Download,
  ListFilter,
  Loader2,
  RefreshCw,
  SlidersHorizontal,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Skeleton } from "@/components/ui/skeleton";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { escapeClaimedElsewhere } from "@/lib/escape-guard";
import { ProviderLogo } from "@/components/chat/provider-logo";
import { SegmentedControl } from "./settings-primitives";
import { UsageSparkline, UsageTrendChart, busiestIndex } from "./usage-trend-chart";
import { UsageTokenTypes, tokenTypeRows } from "./usage-token-types";
import { TermHint } from "./term-hint";
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
import { filterQuota, filterUsageSummary } from "./usage-filter";
import {
  formatCount,
  formatMoney,
  formatPercent,
  formatTokens,
  isKnownProvider,
  own,
  seriesColor,
  seriesFill,
  seriesLabel,
} from "./usage-format";
import { UsageLimitsView, formatCheckedAgo, formatResetsIn } from "./usage-limits";
import {
  ModelPricesDialog,
  PriceEditorDialog,
  UsageModelDialog,
} from "./usage-model-dialog";
import {
  readUsagePreferences,
  saveUsagePreferences,
  type UsageMetric,
  type UsagePreferences,
  type UsageTimeView,
} from "./usage-preferences";

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

type Metric = UsageMetric;

/** One switch for what the page shows: spend, tokens, or plan limits. */
type UsageMode = Metric | "limits";

const MODE_OPTIONS: { value: UsageMode; label: string; key: string }[] = [
  { value: "cost", label: "Cost", key: "c" },
  { value: "tokens", label: "Tokens", key: "t" },
  { value: "limits", label: "Limits", key: "l" },
];

const PERIOD_OPTIONS: { value: UsagePeriod; label: string; key: string }[] = [
  { value: "today", label: "Past 24h", key: "1" },
  { value: "7d", label: "7 days", key: "2" },
  { value: "30d", label: "30 days", key: "3" },
  { value: "90d", label: "90 days", key: "4" },
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
 * model, and session, read from each provider's local history.
 *
 * Cost is always an API/list-price equivalent. Subscription quota is shown
 * separately when a provider reports it; the page never guesses what was
 * actually billed.
 *
 * Also the body of the Usage page, which already titles itself — there
 * `showTitle` is off and only the range line and controls remain. Only the
 * page takes the single-key `shortcuts`; Settings has its own keys.
 */
export function UsageSection({
  showTitle = true,
  shortcuts = false,
}: { showTitle?: boolean; shortcuts?: boolean } = {}) {
  const [preferences, setPreferences] = useState<UsagePreferences>(readUsagePreferences);
  const { view, period, metric, timeView } = preferences;
  const updatePreferences = useCallback((patch: Partial<UsagePreferences>) => {
    setPreferences((current) => {
      const next = { ...current, ...patch };
      saveUsagePreferences(next);
      return next;
    });
  }, []);
  const setPeriod = useCallback(
    (value: UsagePeriod) => updatePreferences({ period: value }),
    [updatePreferences],
  );
  const setMode = useCallback(
    (mode: UsageMode) =>
      updatePreferences(
        mode === "limits" ? { view: "limits" } : { view: "activity", metric: mode },
      ),
    [updatePreferences],
  );
  const setTimeView = (value: UsageTimeView) => updatePreferences({ timeView: value });
  const hiddenProviders = useMemo(
    () => new Set(preferences.hiddenProviders),
    [preferences.hiddenProviders],
  );
  const mode: UsageMode = view === "limits" ? "limits" : metric;

  const [rawSummary, setRawSummary] = useState<UsageSummary | null>(null);
  const [error, setError] = useState<string | null>(null);
  // One pointer for the whole page: a day, provider or model pointed at
  // anywhere lights up in every chart and table that shows it.
  const [hovered, setHovered] = useState<number | null>(null);
  const [hoveredProvider, setHoveredProvider] = useState<string | null>(null);
  const [hoveredModel, setHoveredModel] = useState<FlatModelUsage | null>(null);
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
      .then((next) => setRawSummary(next))
      .catch((err) => {
        setRawSummary(null);
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

  // C / T / L pick the view and 1–4 the period, as on t3code — only while
  // nothing else (a field, menu, or dialog) owns the key.
  useEffect(() => {
    if (!shortcuts) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.metaKey || event.ctrlKey || event.altKey || event.repeat) return;
      if (escapeClaimedElsewhere(event)) return;
      const key = event.key.toLowerCase();
      const nextMode = MODE_OPTIONS.find((option) => option.key === key)?.value;
      const nextPeriod = PERIOD_OPTIONS.find((option) => option.key === key)?.value;
      if (nextMode) setMode(nextMode);
      else if (nextPeriod) setPeriod(nextPeriod);
      else return;
      event.preventDefault();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [shortcuts, setMode, setPeriod]);

  const summary = useMemo(
    () => (rawSummary ? filterUsageSummary(rawSummary, hiddenProviders) : null),
    [rawSummary, hiddenProviders],
  );
  const overlay = useMemo(
    () =>
      hoveredModel
        ? {
            model: hoveredModel.model,
            values: hoveredModel.buckets.map((slice) => metricOf(slice, metric)),
          }
        : null,
    [hoveredModel, metric],
  );
  const quota = useMemo(
    () => filterQuota(newestQuota(rawSummary?.quota, directQuota), hiddenProviders),
    [rawSummary?.quota, directQuota, hiddenProviders],
  );
  const shownStatuses = useMemo(
    () => quotaStatuses.filter((status) => !hiddenProviders.has(status.provider)),
    [quotaStatuses, hiddenProviders],
  );
  // Every provider the page knows of, hidden ones included, so a hidden
  // provider can always be brought back from the filter.
  const knownProviders = useMemo(() => {
    const ids = new Set<string>([
      ...(rawSummary?.providers.map((p) => p.provider) ?? []),
      ...Object.keys(directQuota ?? {}),
      ...preferences.hiddenProviders,
    ]);
    return [...ids];
  }, [rawSummary, directQuota, preferences.hiddenProviders]);
  const toggleProvider = (provider: string, visible: boolean) => {
    const next = new Set(hiddenProviders);
    if (visible) next.delete(provider);
    else next.add(provider);
    updatePreferences({ hiddenProviders: [...next] });
  };

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
  // The figures on screen belong to another period until the new one
  // lands; they stay up, dimmed, rather than blanking the page.
  const stale = summary !== null && summary.period !== period;
  const newestReading = Math.max(
    0,
    ...Object.values(quota)
      .filter((q) => q.windows.length > 0)
      .map((q) => q.received_at_ms),
  );

  return (
    <div>
      <div className="mb-6 flex flex-wrap items-center justify-between gap-x-4 gap-y-3">
        <div className="min-w-0">
          {showTitle && (
            <h2 className="text-body-lg font-semibold tracking-tight">Usage</h2>
          )}
          <p
            className={cn(
              "flex items-center gap-1.5 text-body-sm text-muted-foreground",
              showTitle && "mt-1",
            )}
          >
            {view === "limits"
              ? quotaLoading
                ? "Plan limits · reading…"
                : newestReading > 0
                  ? `Plan limits · checked ${formatCheckedAgo(newestReading, now)}`
                  : "Plan limits"
              : `${summary ? rangeLabel(summary) : "Loading…"} · this machine · ${
                  scanning ? "reading provider history" : refreshing ? "refreshing" : "live"
                }`}
            {busy && <Loader2 className="size-3 animate-spin" aria-hidden />}
          </p>
        </div>
        <div className="ml-auto flex flex-wrap items-center justify-end gap-2">
          <ProviderFilter
            providers={knownProviders}
            hidden={hiddenProviders}
            onToggle={toggleProvider}
            onShowAll={() => updatePreferences({ hiddenProviders: [] })}
          />
          <SegmentedControl
            value={mode}
            onChange={setMode}
            options={MODE_OPTIONS.map(withShortcutTitle(shortcuts))}
            ariaLabel="Usage view"
          />
          {/* The period does not apply to Limits; it stays in place but
              disabled, so switching views does not shift the controls. */}
          <SegmentedControl
            value={period}
            onChange={setPeriod}
            options={PERIOD_OPTIONS.map(withShortcutTitle(shortcuts))}
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
            aria-label="Export CSV"
            title="Export CSV"
          >
            <Download className="size-3.5" aria-hidden />
            {/* Icon-only until the header has room for the words. */}
            <span className="hidden xl:inline">Export CSV</span>
          </Button>
        </div>
      </div>

      {view === "limits" ? (
        <UsageLimitsView
          quota={quota}
          statuses={shownStatuses}
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
            !error && <UsageSkeleton />
          ) : summary.totals.total_tokens === 0 && hiddenProviders.size === 0 ? (
            <div className="rounded-lg border border-border/60 bg-muted/30 px-4 py-10 text-center text-body text-muted-foreground">
              No agent activity in this period.
            </div>
          ) : (
            <div
              aria-busy={stale || undefined}
              className={cn(
                "flex flex-col gap-10 transition-opacity duration-250",
                stale && "opacity-45 delay-150",
              )}
            >
              <HeadlineSection
                summary={summary}
                metric={metric}
                onOpenPrices={() => setPricesOpen(true)}
              />
              <OverTimeSection
                summary={summary}
                metric={metric}
                view={timeView}
                onViewChange={setTimeView}
                hovered={hovered}
                onHover={setHovered}
                hoveredProvider={hoveredProvider}
                onHoverProvider={setHoveredProvider}
                overlay={overlay}
              />
              {summary.providers.length > 0 && (
                <ToolsSection
                  summary={summary}
                  quota={quota}
                  metric={metric}
                  hovered={hovered}
                  onHover={setHovered}
                  hoveredProvider={hoveredProvider}
                  onHoverProvider={setHoveredProvider}
                  onHoverModel={setHoveredModel}
                  onSelectModel={(model) =>
                    setSelectedModelKey(`${model.provider}:${model.model}`)
                  }
                  onOpenPrices={() => setPricesOpen(true)}
                  onOpenLimits={() => setMode("limits")}
                />
              )}
              <TokenTypesSection summary={summary} metric={metric} />
              <ProviderHistoryFooter
                sessionCount={summary.totals.session_count}
                providers={summary.providers.map((p) => p.provider)}
              />
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
          knownModels={rawSummary?.models.map((m) => m.model) ?? []}
          onClose={() => setPriceEditor(null)}
          onSaved={onPricesChanged}
        />
      )}
    </div>
  );
}

/** Adds "(key)" to each option's hover title when shortcuts are live. */
function withShortcutTitle(enabled: boolean) {
  return <T extends string>(option: { value: T; label: string; key: string }) => ({
    value: option.value,
    label: option.label,
    title: enabled ? `${option.label} (${option.key.toUpperCase()})` : undefined,
  });
}

/** Which providers the page counts. Hiding one removes it from every
 *  figure, chart, and table, and from Limits. The last visible provider
 *  cannot be hidden, so the page is never filtered down to nothing. */
function ProviderFilter({
  providers,
  hidden,
  onToggle,
  onShowAll,
}: {
  providers: string[];
  hidden: ReadonlySet<string>;
  onToggle: (provider: string, visible: boolean) => void;
  onShowAll: () => void;
}) {
  if (providers.length < 2) return null;
  const visibleCount = providers.filter((p) => !hidden.has(p)).length;
  const label =
    visibleCount === providers.length
      ? "All providers"
      : visibleCount === 1
        ? seriesLabel(providers.find((p) => !hidden.has(p)) ?? "")
        : `${visibleCount} of ${providers.length} providers`;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          aria-label={`Filter providers: ${label}`}
          className={cn(visibleCount < providers.length && "text-foreground")}
        >
          <ListFilter className="size-3.5" aria-hidden />
          {/* Narrow headers keep just the icon, plus a count when filtered. */}
          <span className="hidden xl:inline">{label}</span>
          {visibleCount < providers.length && (
            <span className="font-mono text-caption tabular-nums xl:hidden">
              {visibleCount}/{providers.length}
            </span>
          )}
          <ChevronDown className="size-3 text-muted-foreground" aria-hidden />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-52">
        {providers.map((provider) => {
          const visible = !hidden.has(provider);
          return (
            <DropdownMenuCheckboxItem
              key={provider}
              checked={visible}
              disabled={visible && visibleCount === 1}
              onSelect={(event) => event.preventDefault()}
              onCheckedChange={(checked) => onToggle(provider, checked === true)}
            >
              <span
                className={cn("size-2 shrink-0 rounded-full", seriesFill(provider))}
                aria-hidden
              />
              {isKnownProvider(provider) ? (
                <ProviderLogo provider={provider} className="size-3.5" />
              ) : null}
              {seriesLabel(provider)}
            </DropdownMenuCheckboxItem>
          );
        })}
        {visibleCount < providers.length && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={onShowAll}>Show all providers</DropdownMenuItem>
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** The page's own shape while the first summary loads, so nothing jumps
 *  when it lands. */
function UsageSkeleton() {
  return (
    <div className="flex flex-col gap-10" aria-busy aria-label="Loading usage">
      <div className="flex flex-wrap justify-between gap-6">
        <div className="flex flex-col gap-2">
          <Skeleton className="h-3 w-28" />
          <Skeleton className="h-10 w-64" />
          <Skeleton className="h-4 w-80" />
        </div>
        <div className="flex w-[17rem] flex-col gap-2">
          <Skeleton className="h-3 w-24" />
          <Skeleton className="h-2 w-full" />
          <Skeleton className="h-12 w-full" />
        </div>
      </div>
      <div className="flex flex-col gap-3">
        <Skeleton className="h-3 w-20" />
        <Skeleton className="h-5 w-72" />
        <Skeleton className="h-[232px] w-full" />
      </div>
      <Skeleton className="h-56 w-full rounded-lg" />
    </div>
  );
}

/** First bucket to last, in the backend's own words. "Past 24h" is a
 *  trailing 24-hour window rather than a calendar day, so it reads
 *  "Yesterday 13:00 – Today 12:00" — naming it "Today, 00:00 – now" would
 *  claim a range the chart does not show. */
function rangeLabel(summary: UsageSummary): string {
  const first = summary.buckets[0];
  const last = summary.buckets[summary.buckets.length - 1];
  if (!first || !last) return "";
  return `${first.sub_label} – ${last.sub_label}`;
}

const PERIOD_WORDS: Record<UsagePeriod, string> = {
  today: "past 24 hours",
  "7d": "last 7 days",
  "30d": "last 30 days",
  "90d": "last 90 days",
};

/** The value one bucket contributes for one provider, under the active
 *  metric. Shared by the chart and its readout so a hovered point and
 *  its numbers can never disagree. */
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

/** Y-axis figures: whole dollars wherever cents add nothing ($0, $20,
 *  $1,500), so the axis gutter stays narrow. The readout keeps the cents. */
function formatAxisValue(value: number, metric: Metric): string {
  if (metric === "tokens") return formatTokens(value);
  return value >= 100 || Number.isInteger(value)
    ? `$${Math.round(value).toLocaleString("en-US")}`
    : formatMoney(value);
}

function metricOf(item: { cost_usd: number; tokens: number }, metric: Metric): number {
  return metric === "cost" ? item.cost_usd : item.tokens;
}

/** What one section's answer line and title look like: a quiet label,
 *  then the finding in plain words, so the chart below is the evidence
 *  rather than the puzzle. */
function SectionHeading({
  title,
  answer,
  hint,
  children,
}: {
  title: string;
  answer?: ReactNode;
  /** How to read or use the section, under the answer. */
  hint?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <div className="flex flex-wrap items-end justify-between gap-x-4 gap-y-2">
      <div className="flex min-w-0 flex-col gap-1">
        <Eyebrow>{title}</Eyebrow>
        {answer && <p className="text-body text-foreground">{answer}</p>}
        {hint && <p className="text-body-sm text-muted-foreground">{hint}</p>}
      </div>
      {children && <div className="flex flex-wrap items-center gap-2">{children}</div>}
    </div>
  );
}

// ── headline ──

/** The one number the page is about, what it is (and is not), and how
 *  much of it rests on known prices. */
function HeadlineSection({
  summary,
  metric,
  onOpenPrices,
}: {
  summary: UsageSummary;
  metric: Metric;
  onOpenPrices: () => void;
}) {
  const { totals } = summary;
  const total = metric === "cost" ? totals.estimated_cost_usd : totals.total_tokens;
  return (
    <section className="flex flex-wrap items-start justify-between gap-x-10 gap-y-6">
      <div className="flex min-w-0 flex-col gap-2">
        <Eyebrow>{metric === "cost" ? "Estimated API value" : "Tokens processed"}</Eyebrow>
        <p className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
          <span className="select-text font-mono text-[2.25rem] font-semibold leading-none tabular-nums tracking-tight text-foreground">
            {formatMetric(total, metric)}
          </span>
          <span className="text-body text-muted-foreground">
            across {formatCount(totals.session_count)}{" "}
            {totals.session_count === 1 ? "session" : "sessions"} ·{" "}
            {PERIOD_WORDS[summary.period as UsagePeriod] ?? rangeLabel(summary)}
          </span>
        </p>
        <p className="max-w-xl text-body-sm text-foreground/80">
          {metric === "cost" ? (
            <>
              Not your bill: what this work would cost at each provider&apos;s{" "}
              <TermHint hint="Subscriptions such as Claude Max or ChatGPT Pro are billed differently. Switch to Limits to see how much of your plan is left.">
                public API prices
              </TermHint>
              .
            </>
          ) : (
            <>Every token the models read or wrote: new input, reused context, cache writes and output.</>
          )}
        </p>
        {metric === "cost" && summary.confidence.unpriced_token_share > 0 && (
          <p className="text-body-sm text-status-working">
            Excludes {formatPercent(summary.confidence.unpriced_token_share)} of tokens from
            models with no known price.{" "}
            <button
              type="button"
              onClick={onOpenPrices}
              className="rounded-sm font-medium text-foreground underline underline-offset-[3px] hover:text-accent-ember focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/60"
            >
              Set prices
            </button>
          </p>
        )}
      </div>
      {metric === "cost" ? (
        <PricingCoverage confidence={summary.confidence} />
      ) : (
        <div className="flex w-[15rem] flex-col gap-1.5">
          <Eyebrow>Reused from cache</Eyebrow>
          <span className="font-mono text-body-lg font-semibold tabular-nums">
            {formatPercent(totals.cache_read_share)}
          </span>
          <span className="text-body-sm text-muted-foreground">
            of all tokens were earlier context the model{" "}
            <TermHint hint="A cache read re-uses conversation the provider already holds, at about a tenth of the input price.">
              re-read cheaply
            </TermHint>
            .
          </span>
        </div>
      )}
    </section>
  );
}

/** Where the estimate's prices came from — so a reader can tell how much
 *  of the headline is the provider's own figure, Codemux's price list, or
 *  their own rates, and what was left out. */
function PricingCoverage({ confidence }: { confidence: CostConfidence }) {
  const sources = [
    {
      key: "provider",
      label: "Reported by the provider",
      share: confidence.provider_reported_share,
      tone: "bg-foreground/85",
      hint: "Cost the provider reported itself.",
    },
    {
      key: "table",
      label: "Codemux price list",
      share: confidence.table_priced_share,
      tone: "bg-foreground/50",
      hint: "Priced from the public list prices Codemux knows.",
    },
    {
      key: "override",
      label: "Your prices",
      share: confidence.override_priced_share ?? 0,
      tone: "bg-accent-ember",
      hint: "Priced from rates you set under Model prices.",
    },
  ];
  const lead = [...sources].sort((a, b) => b.share - a.share)[0]!;
  return (
    <div className="flex w-[17rem] flex-col gap-2">
      <Eyebrow>How it&apos;s priced</Eyebrow>
      <div className="flex h-2 gap-0.5 overflow-hidden rounded-full bg-muted/50" role="img" aria-label={sources.map((s) => `${s.label} ${formatPercent(s.share)}`).join(", ")}>
        {sources
          .filter((s) => s.share > 0)
          .map((s) => (
            <Tooltip key={s.key}>
              <TooltipTrigger asChild>
                <span
                  className={cn("h-full first:rounded-l-full last:rounded-r-full", s.tone)}
                  style={{ width: `${s.share * 100}%` }}
                />
              </TooltipTrigger>
              <TooltipContent side="top">
                {s.label}: {formatPercent(s.share)} of cost. {s.hint}
              </TooltipContent>
            </Tooltip>
          ))}
      </div>
      <ul className="flex flex-col gap-1 text-body-sm">
        {sources.map((s) => (
          <li key={s.key} className="flex items-center justify-between gap-3">
            <span className="flex items-center gap-2 text-muted-foreground">
              <span className={cn("size-2 shrink-0 rounded-full", s.tone)} aria-hidden />
              {s.label}
            </span>
            <span
              className={cn(
                "font-mono tabular-nums",
                s.key === lead.key ? "text-foreground" : "text-muted-foreground",
              )}
            >
              {formatPercent(s.share)}
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}

// ── over time ──

/** The chart flexes horizontally on its own; only the height needs
 *  stating. */
const CHART_HEIGHT_PX = 232;

const TIME_VIEW_OPTIONS: { value: UsageTimeView; label: string }[] = [
  { value: "chart", label: "Chart" },
  { value: "table", label: "Table" },
];

function OverTimeSection({
  summary,
  metric,
  view,
  onViewChange,
  hovered,
  onHover,
  hoveredProvider,
  onHoverProvider,
  overlay,
}: {
  summary: UsageSummary;
  metric: Metric;
  view: UsageTimeView;
  onViewChange: (view: UsageTimeView) => void;
  hovered: number | null;
  onHover: (index: number | null) => void;
  hoveredProvider: string | null;
  onHoverProvider: (provider: string | null) => void;
  /** One model's part of each bucket, while that model is pointed at. */
  overlay: { model: string; values: number[] } | null;
}) {
  const { providers, buckets, models } = summary;
  const hourly = summary.period === "today";
  // Largest provider first, in the legend, the readout and the table.
  const order = useMemo(
    () => [...providers].sort((a, b) => metricOf(b, metric) - metricOf(a, metric)),
    [providers, metric],
  );
  const series = useMemo(
    () =>
      order.map((p) => ({
        key: p.provider,
        label: seriesLabel(p.provider),
        ...seriesColor(p.provider),
        values: buckets.map((bucket) => bucketValue(bucket, p.provider, metric)),
      })),
    [buckets, order, metric],
  );
  const points = useMemo(
    () => buckets.map((b) => ({ label: b.label, subLabel: b.sub_label })),
    [buckets],
  );
  const busiest = busiestIndex(series, buckets.length);
  const periodTotal = metric === "cost" ? summary.totals.estimated_cost_usd : summary.totals.total_tokens;
  const busiestTotal =
    busiest === null ? 0 : series.reduce((sum, s) => sum + (s.values[busiest] ?? 0), 0);
  const unit = hourly ? "hour" : "day";
  // The chart memoises its geometry on these, so they must stay stable
  // across hover re-renders.
  const formatValue = useCallback((v: number) => formatMetric(v, metric), [metric]);
  const formatAxis = useCallback((v: number) => formatAxisValue(v, metric), [metric]);
  const secondaryValue = useCallback(
    (provider: string, index: number) => {
      const slice = buckets[index]?.providers[provider];
      if (!slice) return null;
      return metric === "cost" ? `${formatTokens(slice.tokens)} tok` : formatMoney(slice.cost_usd);
    },
    [buckets, metric],
  );

  const topModel = (index: number) => {
    let best: FlatModelUsage | null = null;
    let bestValue = 0;
    for (const m of models) {
      const value = metricOf(m.buckets[index] ?? { cost_usd: 0, tokens: 0 }, metric);
      if (value > bestValue) {
        best = m;
        bestValue = value;
      }
    }
    if (!best) return null;
    return (
      <p className="mt-1.5 flex items-center justify-between gap-4 border-t border-border/50 pt-1.5 text-label text-muted-foreground">
        <span>Top model</span>
        <span className="truncate font-mono text-foreground">
          {best.model} · {formatMetric(bestValue, metric)}
        </span>
      </p>
    );
  };

  return (
    <section className="flex flex-col gap-4">
      <SectionHeading
        title="Over time"
        answer={
          busiest === null ? (
            <span className="text-muted-foreground">Nothing ran in this period.</span>
          ) : (
            <>
              Busiest {unit}:{" "}
              <span className="font-medium">{buckets[busiest]?.sub_label}</span> ·{" "}
              <span className="font-mono">{formatMetric(busiestTotal, metric)}</span>
              <span className="text-muted-foreground">
                , {formatPercent(periodTotal > 0 ? busiestTotal / periodTotal : 0)} of the period
              </span>
            </>
          )
        }
      >
        <ul className="flex flex-wrap items-center gap-1" aria-label="Providers in the chart">
          {order.map((p) => (
            <li key={p.provider}>
              <button
                type="button"
                onPointerEnter={() => onHoverProvider(p.provider)}
                onPointerLeave={() => onHoverProvider(null)}
                onFocus={() => onHoverProvider(p.provider)}
                onBlur={() => onHoverProvider(null)}
                className={cn(
                  "inline-flex items-center gap-1.5 rounded-md px-2 py-1 text-label transition-colors duration-150 hover:bg-accent/30 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/60",
                  hoveredProvider && hoveredProvider !== p.provider
                    ? "text-muted-foreground/60"
                    : "text-foreground",
                )}
              >
                <span
                  className={cn("size-2 shrink-0 rounded-full", seriesFill(p.provider))}
                  aria-hidden
                />
                {seriesLabel(p.provider)}
              </button>
            </li>
          ))}
        </ul>
        <SegmentedControl
          value={view}
          onChange={onViewChange}
          options={TIME_VIEW_OPTIONS}
          ariaLabel="Over time view"
          size="sm"
        />
      </SectionHeading>

      {view === "chart" && (
        <p className="-mt-2 h-5 text-body-sm text-muted-foreground" aria-live="polite">
          {overlay ? (
            <>
              Dashed line: <span className="font-mono text-foreground">{overlay.model}</span>{" "}
              on its own
            </>
          ) : (
            <>Point at a {unit} for its breakdown, or at a tool or model below to trace it here.</>
          )}
        </p>
      )}
      {view === "chart" ? (
        <UsageTrendChart
          series={series}
          points={points}
          height={CHART_HEIGHT_PX}
          hovered={hovered}
          onHover={onHover}
          highlightedSeries={hoveredProvider}
          formatValue={formatValue}
          formatAxis={formatAxis}
          secondaryValue={secondaryValue}
          readoutExtra={topModel}
          overlay={overlay?.values ?? null}
          ariaLabel={
            metric === "cost"
              ? `Estimated API value per ${unit} by provider`
              : `Tokens per ${unit} by provider`
          }
        />
      ) : (
        <DayRows
          summary={summary}
          order={order.map((p) => p.provider)}
          metric={metric}
          hovered={hovered}
          onHover={onHover}
        />
      )}
    </section>
  );
}

function DayRows({
  summary,
  order,
  metric,
  hovered,
  onHover,
}: {
  summary: UsageSummary;
  /** Provider columns, largest first, like the chart legend. */
  order: string[];
  metric: Metric;
  hovered: number | null;
  onHover: (index: number | null) => void;
}) {
  // Derived from the buckets the chart already uses, so the two can
  // never disagree. Newest first — the opposite of the chart's axis,
  // because a table is read from the top. One column per provider under
  // the active metric, so a spike can be traced to whoever caused it;
  // the last column carries the other metric.
  const rows = summary.buckets
    .map((bucket, index) => {
      const providers = Object.values(bucket.providers);
      return {
        index,
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
      <p className="border-t border-border/60 py-6 text-center text-body-sm text-muted-foreground">
        No activity in this period.
      </p>
    );
  }
  const columns = `minmax(5.5rem,1fr) repeat(${order.length}, minmax(4.5rem,5.5rem)) 6rem 5rem`;
  return (
    <div className="flex flex-col overflow-x-auto" onPointerLeave={() => onHover(null)}>
      <p className="pb-1 text-body-sm text-muted-foreground">
        Each tool&apos;s {metric === "cost" ? "estimated API value" : "tokens"} per{" "}
        {summary.period === "today" ? "hour" : "day"}, newest first.
      </p>
      <div
        className="grid items-center gap-x-3 border-b border-border/60 py-2 text-label text-muted-foreground"
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
        <span className="text-right">{metric === "cost" ? "Total value" : "Total tokens"}</span>
        <span className="text-right">{metric === "cost" ? "Tokens" : "Est. value"}</span>
      </div>
      {rows.map((r) => (
        <div
          key={r.key}
          onPointerEnter={() => onHover(r.index)}
          className={cn(
            "grid items-center gap-x-3 border-b border-border/40 py-2 transition-colors duration-150 hover:bg-accent/20",
            hovered === r.index && "bg-accent/20",
          )}
          style={{ gridTemplateColumns: columns }}
        >
          <span className="min-w-0 truncate text-body-sm text-muted-foreground">
            {r.label}
          </span>
          {order.map((provider) => (
            <span
              key={provider}
              className="select-text text-right font-mono text-label tabular-nums text-muted-foreground"
            >
              {r.byProvider[provider]
                ? formatMetric(metricOf(r.byProvider[provider], metric), metric)
                : "—"}
            </span>
          ))}
          <span className="select-text text-right font-mono text-body-sm tabular-nums">
            {metric === "cost" ? formatMoney(r.cost) : formatTokens(r.tokens)}
          </span>
          <span className="select-text text-right font-mono text-label tabular-nums text-muted-foreground">
            {metric === "cost" ? formatTokens(r.tokens) : formatMoney(r.cost)}
          </span>
        </div>
      ))}
    </div>
  );
}

// ── by tool and model ──

/** Models ranked by the active metric. Unpriced models have no cost, so
 *  under Cost they sort after every priced one, by tokens. */
export function sortModels(models: FlatModelUsage[], metric: Metric): FlatModelUsage[] {
  return [...models].sort((a, b) => {
    if (metric === "tokens") return b.tokens - a.tokens || a.model.localeCompare(b.model);
    if (a.priced !== b.priced) return a.priced ? -1 : 1;
    return b.cost_usd - a.cost_usd || b.tokens - a.tokens || a.model.localeCompare(b.model);
  });
}

/** Models listed under each tool before "Show more". */
const MODELS_SHOWN = 3;
const STRIP_HEIGHT_PX = 26;

/** Each tool, with its plan headroom and day-by-day rhythm, and the models
 *  that did its work nested underneath — one place per figure. */
function ToolsSection({
  summary,
  quota,
  metric,
  hovered,
  onHover,
  hoveredProvider,
  onHoverProvider,
  onHoverModel,
  onSelectModel,
  onOpenPrices,
  onOpenLimits,
}: {
  summary: UsageSummary;
  quota: Record<string, ProviderQuota>;
  metric: Metric;
  hovered: number | null;
  onHover: (index: number | null) => void;
  hoveredProvider: string | null;
  onHoverProvider: (provider: string | null) => void;
  onHoverModel: (model: FlatModelUsage | null) => void;
  onSelectModel: (model: FlatModelUsage) => void;
  onOpenPrices: () => void;
  onOpenLimits: () => void;
}) {
  const total = metric === "cost" ? summary.totals.estimated_cost_usd : summary.totals.total_tokens;
  const lanes = useMemo(
    () => [...summary.providers].sort((a, b) => metricOf(b, metric) - metricOf(a, metric)),
    [summary.providers, metric],
  );
  const topModel = sortModels(summary.models, metric)[0];
  // One scale for every model bar on the page, so bars compare across tools.
  const modelPeak = Math.max(0, ...summary.models.map((m) => metricOf(m, metric)));
  const lead = lanes[0];
  const share = (value: number) => (total > 0 ? value / total : 0);

  return (
    <section className="flex flex-col gap-4">
      <SectionHeading
        title="By tool and model"
        answer={
          lead && (
            <>
              {lanes.length === 1 ? (
                <>All of it came from {seriesLabel(lead.provider)}</>
              ) : (
                <>
                  {seriesLabel(lead.provider)} did{" "}
                  <span className="font-mono">{formatPercent(share(metricOf(lead, metric)))}</span> of it
                </>
              )}
              {topModel && (topModel.priced || metric === "tokens") && (
                <span className="text-muted-foreground">
                  ; <span className="font-mono text-foreground">{topModel.model}</span> was the
                  largest model at{" "}
                  <span className="font-mono text-foreground">
                    {formatMetric(metricOf(topModel, metric), metric)}
                  </span>
                </span>
              )}
              .
            </>
          )
        }
        hint={
          <>
            The small line beside each tool shows its activity per{" "}
            {summary.period === "today" ? "hour" : "day"} on its own scale; model bars share one
            scale. Select a model for its trend and prices.
          </>
        }
      >
        <Button variant="ghost" size="sm" onClick={onOpenPrices}>
          <SlidersHorizontal className="size-3.5" aria-hidden />
          Model prices
        </Button>
      </SectionHeading>

      <div className="divide-y divide-border/50 rounded-lg border border-border/60">
        {lanes.map((provider) => (
          <ToolGroup
            key={provider.provider}
            provider={provider}
            models={sortModels(
              summary.models.filter((m) => m.provider === provider.provider),
              metric,
            )}
            quota={quota[provider.provider]}
            buckets={summary.buckets}
            metric={metric}
            pageTotal={total}
            pageTokens={summary.totals.total_tokens}
            modelPeak={modelPeak}
            unit={summary.period === "today" ? "hour" : "day"}
            hovered={hovered}
            onHover={onHover}
            dimmed={hoveredProvider !== null && hoveredProvider !== provider.provider}
            onHoverProvider={onHoverProvider}
            onHoverModel={onHoverModel}
            onSelectModel={onSelectModel}
            onOpenLimits={onOpenLimits}
          />
        ))}
      </div>
    </section>
  );
}

function ToolGroup({
  provider,
  models,
  quota,
  buckets,
  metric,
  pageTotal,
  pageTokens,
  modelPeak,
  unit,
  hovered,
  onHover,
  dimmed,
  onHoverProvider,
  onHoverModel,
  onSelectModel,
  onOpenLimits,
}: {
  provider: UsageProvider;
  models: FlatModelUsage[];
  /** Live plan quota, when the provider reports any. */
  quota?: ProviderQuota;
  buckets: UsageBucket[];
  metric: Metric;
  pageTotal: number;
  pageTokens: number;
  /** The largest model on the page, which every model bar is scaled to. */
  modelPeak: number;
  unit: "hour" | "day";
  hovered: number | null;
  onHover: (index: number | null) => void;
  dimmed: boolean;
  onHoverProvider: (provider: string | null) => void;
  onHoverModel: (model: FlatModelUsage | null) => void;
  onSelectModel: (model: FlatModelUsage) => void;
  onOpenLimits: () => void;
}) {
  const [showAll, setShowAll] = useState(false);
  const bars = quota ? meterWindows(quota.windows) : [];
  const now = Date.now();
  const values = useMemo(
    () => buckets.map((b) => bucketValue(b, provider.provider, metric)),
    [buckets, provider.provider, metric],
  );
  const value = metricOf(provider, metric);
  const share = pageTotal > 0 ? value / pageTotal : 0;
  const subagentTokens = new Map(provider.models.map((m) => [m.model, m.subagent_tokens]));
  const shown = showAll ? models : models.slice(0, MODELS_SHOWN);

  return (
    <div
      className={cn("transition-opacity duration-150", dimmed && "opacity-50")}
      onPointerEnter={() => onHoverProvider(provider.provider)}
      onPointerLeave={() => onHoverProvider(null)}
    >
      <div className="grid grid-cols-[minmax(10rem,13rem)_minmax(0,1fr)_auto] items-center gap-x-6 gap-y-3 px-4 py-3.5 lg:grid-cols-[minmax(10rem,13rem)_11.5rem_minmax(0,1fr)_auto]">
        <span className="flex min-w-0 items-center gap-2.5">
          {isKnownProvider(provider.provider) ? (
            <ProviderLogo provider={provider.provider} className="h-[18px] w-[18px] shrink-0" />
          ) : (
            <span
              className={cn("size-2.5 shrink-0 rounded-full", seriesFill(provider.provider))}
              aria-hidden
            />
          )}
          <span className="flex min-w-0 flex-col gap-0.5">
            <span className="truncate text-body font-medium">{seriesLabel(provider.provider)}</span>
            <span className="truncate text-label text-muted-foreground">
              {quota?.plan_label ? `${quota.plan_label} · ` : ""}
              {formatCount(provider.session_count)}{" "}
              {provider.session_count === 1 ? "session" : "sessions"}
            </span>
          </span>
        </span>

        {/* Plan headroom in the same words as Limits: the bar is what is
            LEFT, so a short bar always means "close to the cap". */}
        {bars.length > 0 ? (
          <button
            type="button"
            onClick={onOpenLimits}
            aria-label={`${seriesLabel(provider.provider)} plan limits: ${bars
              .map((w) => `${WINDOW_LABEL[w.kind]} ${remainingPct(w)}% left`)
              .join(", ")}. Open Limits.`}
            className="hidden flex-col gap-1.5 rounded-md text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/60 lg:flex"
          >
            {bars.map((w) => (
              <Tooltip key={w.kind}>
                <TooltipTrigger asChild>
                  <span className="flex items-center gap-2">
                    <span className="w-9 shrink-0 text-label text-muted-foreground">
                      {WINDOW_LABEL[w.kind]}
                    </span>
                    <span className="h-1.5 flex-1 overflow-hidden rounded-full bg-muted-foreground/20">
                      <span
                        className={cn("block h-full rounded-full", meterTone(w.used_pct))}
                        style={{ width: `${remainingPct(w)}%` }}
                      />
                    </span>
                    <span className="w-14 shrink-0 text-right font-mono text-label tabular-nums text-muted-foreground">
                      {remainingPct(w)}% left
                    </span>
                  </span>
                </TooltipTrigger>
                <TooltipContent side="top">
                  {WINDOW_LONG_LABEL[w.kind]}: {Math.round(w.used_pct)}% used, {remainingPct(w)}%
                  left{formatResetAt(w.resets_at_ms) ? ` · ${formatResetAt(w.resets_at_ms)}` : ""}.
                  Click for all limits.
                </TooltipContent>
              </Tooltip>
            ))}
            {resetNotes(bars, now).map((note) => (
              <span key={note} className="pl-11 text-label text-foreground/75">
                {note}
              </span>
            ))}
          </button>
        ) : (
          <span className="hidden text-label text-muted-foreground lg:block">
            No plan limits reported
          </span>
        )}

        <UsageSparkline
          values={values}
          height={STRIP_HEIGHT_PX}
          hovered={hovered}
          onHover={onHover}
          ariaLabel={`${seriesLabel(provider.provider)} per ${unit}`}
          describe={(index) => {
            const slice = buckets[index]?.providers[provider.provider];
            const label = buckets[index]?.sub_label ?? "";
            if (!slice) return `${label} · nothing`;
            return `${label} · ${formatMoney(slice.cost_usd)} · ${formatTokens(slice.tokens)} tokens`;
          }}
          {...seriesColor(provider.provider)}
        />

        <span className="flex items-baseline gap-4 justify-self-end">
          <span className="flex flex-col items-end gap-0.5">
            <span className="font-mono text-body tabular-nums">{formatPercent(share)}</span>
            <span className="text-label text-foreground/70">
              of {metric === "cost" ? "value" : "tokens"}
            </span>
          </span>
          <span className="flex w-[6.5rem] flex-col items-end gap-0.5">
            <span className="select-text font-mono text-body-lg font-semibold tabular-nums tracking-tight">
              {formatMetric(value, metric)}
            </span>
            <span className="text-label text-muted-foreground">
              {metric === "cost"
                ? `${formatTokens(provider.tokens)} tokens`
                : formatMoney(provider.cost_usd)}
            </span>
          </span>
        </span>
      </div>

      {models.length > 0 && (
        <ul className="pb-2" aria-label={`${seriesLabel(provider.provider)} models`}>
          {shown.map((m) => {
            const modelValue = metricOf(m, metric);
            const subagents = (subagentTokens.get(m.model) ?? 0) > 0;
            return (
              <li key={m.model}>
                <button
                  type="button"
                  onClick={() => onSelectModel(m)}
                  onPointerEnter={() => onHoverModel(m)}
                  onPointerLeave={() => onHoverModel(null)}
                  onFocus={() => onHoverModel(m)}
                  onBlur={() => onHoverModel(null)}
                  aria-label={`Open ${m.model} details`}
                  className="group grid w-full grid-cols-[minmax(0,1fr)_6rem_4.5rem_6.5rem_1rem] items-center gap-x-4 py-1.5 pl-[3.25rem] pr-4 text-left transition-colors duration-150 hover:bg-accent/20 focus-visible:bg-accent/20 focus-visible:outline-none"
                >
                  <span className="flex min-w-0 items-center gap-2">
                    <span className="truncate font-mono text-body-sm text-foreground/90">{m.model}</span>
                    {m.price_overridden && <Pill>your price</Pill>}
                    {!m.priced && <Pill>no price</Pill>}
                    {subagents && <Pill>subagents</Pill>}
                  </span>
                  <span className="h-1.5 overflow-hidden rounded-full bg-muted/50" aria-hidden>
                    <span
                      className={cn("block h-full rounded-full", seriesFill(m.provider))}
                      style={{
                        width:
                          modelValue > 0 && modelPeak > 0
                            ? `max(4%, ${(modelValue / modelPeak) * 100}%)`
                            : 0,
                      }}
                    />
                  </span>
                  <span className="text-right font-mono text-body-sm tabular-nums text-foreground/75">
                    {/* An unpriced model has no cost share, so under Cost it
                        shows its share of the page's tokens, marked "tok". */}
                    {metric === "cost" && !m.priced
                      ? `${formatPercent(pageTokens > 0 ? m.tokens / pageTokens : 0)} tok`
                      : formatPercent(pageTotal > 0 ? modelValue / pageTotal : 0)}
                  </span>
                  <span className="select-text text-right font-mono text-body-sm tabular-nums">
                    {metric === "cost" ? (
                      m.priced ? (
                        formatMoney(m.cost_usd)
                      ) : (
                        <span className="font-sans text-label text-muted-foreground">No price</span>
                      )
                    ) : (
                      formatTokens(m.tokens)
                    )}
                  </span>
                  <ChevronRight
                    className="size-3.5 text-muted-foreground/60 transition-colors duration-150 group-hover:text-foreground group-focus-visible:text-foreground"
                    aria-hidden
                  />
                </button>
              </li>
            );
          })}
          {models.length > MODELS_SHOWN && (
            <li className="pl-[3.25rem] pt-1">
              <button
                type="button"
                onClick={() => setShowAll((v) => !v)}
                className="rounded-sm text-label text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/60"
              >
                {showAll ? "Show fewer" : `Show ${models.length - MODELS_SHOWN} more`}
              </button>
            </li>
          )}
        </ul>
      )}
    </div>
  );
}

function Pill({ children }: { children: ReactNode }) {
  return (
    <span className="shrink-0 rounded-sm bg-muted/60 px-1.5 py-px text-caption text-muted-foreground">
      {children}
    </span>
  );
}

/** When each shown window frees up again, in words: "5h resets in 2h 13m",
 *  so the meters read without a trip to Limits. */
function resetNotes(windows: PlanUsageWindow[], now: number): string[] {
  return windows.flatMap((window) => {
    const resetsIn = formatResetsIn(window, now);
    return resetsIn ? [`${WINDOW_LABEL[window.kind]} ${resetsIn}`] : [];
  });
}

function remainingPct(window: PlanUsageWindow): number {
  return Math.max(0, Math.min(100, Math.round(100 - window.used_pct)));
}

const WINDOW_LONG_LABEL: Record<PlanWindowKind, string> = {
  five_hour: "5-hour window",
  seven_day: "Weekly window",
  seven_day_opus: "Weekly Opus window",
  seven_day_sonnet: "Weekly Sonnet window",
  overage: "Extra usage",
  other: "Limit",
};

// ── by token type ──

function TokenTypesSection({ summary, metric }: { summary: UsageSummary; metric: Metric }) {
  const rows = tokenTypeRows(summary.composition, summary.category_cost);
  const c = summary.composition;
  const output = rows.find((r) => r.key === "output");
  const totalCost = rows.reduce((sum, r) => sum + r.cost, 0);
  const tokenShare = output && c.processed_tokens > 0 ? (output.tokens ?? 0) / c.processed_tokens : 0;
  const costShare = output && totalCost > 0 ? output.cost / totalCost : 0;
  return (
    <section className="flex flex-col gap-4">
      <SectionHeading
        title="By token type"
        answer={
          output && totalCost > 0 ? (
            <>
              Output was <span className="font-mono">{formatPercent(tokenShare)}</span> of tokens
              but <span className="font-mono">{formatPercent(costShare)}</span> of the value
              <span className="text-muted-foreground">: writing costs far more than reading.</span>
            </>
          ) : undefined
        }
      />
      <UsageTokenTypes rows={rows} emphasis={metric} />
      {c.cache_savings_usd > 0 && (
        <p className="flex flex-wrap items-baseline gap-x-1.5 rounded-lg bg-status-open/10 px-4 py-3 text-body-sm text-foreground">
          <span>
            Caching saved about{" "}
            <span className="font-mono font-semibold">{formatMoney(c.cache_savings_usd)}</span>
            {c.cache_savings_multiplier != null && (
              <>
                {" "}
                — the same work would have cost{" "}
                <span className="font-mono">{c.cache_savings_multiplier.toFixed(1)}×</span> as much
                without it
              </>
            )}
            .
          </span>
          <TermHint hint="Compares what reused context actually cost with paying the full input price for it. Only models in Codemux's price list are counted, so tools priced by the provider (such as OpenCode) add nothing here.">
            How is this worked out?
          </TermHint>
        </p>
      )}
    </section>
  );
}

// ── provider-history footer ──

function ProviderHistoryFooter({
  sessionCount,
  providers,
}: {
  sessionCount: number;
  providers: string[];
}) {
  const names = providers.map(seriesLabel);
  const list =
    names.length <= 1
      ? (names[0] ?? "")
      : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
  return (
    <div className="border-t border-border/60 pt-4">
      <p className="text-body-sm leading-relaxed text-muted-foreground">
        <span className="font-medium text-foreground">
          Includes {sessionCount.toLocaleString()} session
          {sessionCount === 1 ? "" : "s"}
          {list ? ` from ${list}` : ""} on this machine
        </span>
        , whichever app launched them. History sources include{" "}
        <span className="font-mono text-caption">~/.claude/projects</span>,{" "}
        <span className="font-mono text-caption">~/.codex/sessions</span>, and
        OpenCode&apos;s local data directory.
      </p>
    </div>
  );
}
