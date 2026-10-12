import { Gauge, Loader2, TrendingDown, TrendingUp, TriangleAlert } from "lucide-react";

import { ProviderLogo } from "@/components/chat/provider-logo";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type {
  PlanUsageWindow,
  PlanWindowKind,
  ProviderQuota,
  QuotaProbeStatus,
} from "@/tauri/commands";
import { formatResetTime } from "@/lib/agent-chat/usage-limit";
import { cn } from "@/lib/utils";
import { isKnownProvider, seriesLabel } from "./usage-format";

const MINUTE_MS = 60_000;
const HOUR_MS = 60 * MINUTE_MS;
const DAY_MS = 24 * HOUR_MS;

/** A reading older than this is shown, but labelled as possibly out of date. */
const STALE_AFTER_MS = 15 * MINUTE_MS;

/** Claude and Codex lead; anything else follows alphabetically. */
const PROVIDER_ORDER = ["claude", "codex"];

const KIND_ORDER: PlanWindowKind[] = [
  "five_hour",
  "seven_day",
  "seven_day_opus",
  "seven_day_sonnet",
  "other",
  "overage",
];

const KIND_MINUTES: Partial<Record<PlanWindowKind, number>> = {
  five_hour: 5 * 60,
  seven_day: 7 * 24 * 60,
  seven_day_opus: 7 * 24 * 60,
  seven_day_sonnet: 7 * 24 * 60,
};

/** Human name for a window: "5-hour", "Weekly · Opus", or the provider's own. */
export function limitWindowLabel(window: PlanUsageWindow): string {
  switch (window.kind) {
    case "five_hour":
      return "5-hour";
    case "seven_day":
      return "Weekly";
    case "seven_day_opus":
      return "Weekly · Opus";
    case "seven_day_sonnet":
      return "Weekly · Sonnet";
    case "overage":
      return window.label && window.label !== "overage" ? window.label : "Extra usage";
    case "other": {
      const label = window.label?.trim();
      if (!label) return "Limit";
      // Codex names unrecognized windows by their length ("1d", "12h").
      return /^\d+[mhd]$/.test(label) ? `${label} window` : label;
    }
  }
}

export function remainingPercent(window: PlanUsageWindow): number {
  return Math.round(100 - Math.max(0, Math.min(100, window.used_pct)));
}

function windowMinutes(window: PlanUsageWindow): number | null {
  return window.window_mins ?? KIND_MINUTES[window.kind] ?? null;
}

/** Elapsed share of the window, 0..1, or null when its length or reset is unknown. */
export function elapsedShare(window: PlanUsageWindow, now: number): number | null {
  const minutes = windowMinutes(window);
  if (window.resets_at_ms == null || minutes == null || minutes <= 0) return null;
  const length = minutes * MINUTE_MS;
  return Math.max(0, Math.min(1, (length - (window.resets_at_ms - now)) / length));
}

export type LimitPace = "ahead" | "on" | "under";

/**
 * Usage against the clock. Spending evenly leaves the same share of quota as
 * there is time left; within five points of that counts as on pace, further
 * ahead means the window may run dry before it resets.
 */
export function paceOf(window: PlanUsageWindow, now: number): LimitPace | null {
  const elapsed = elapsedShare(window, now);
  if (elapsed === null) return null;
  const gap = window.used_pct - elapsed * 100;
  if (gap > 5) return "ahead";
  if (gap < -5) return "under";
  return "on";
}

/** `2h 13m`, `3d 4h`, `12m`. */
export function formatDuration(ms: number): string {
  const remaining = Math.max(0, ms);
  const days = Math.floor(remaining / DAY_MS);
  const hours = Math.floor((remaining % DAY_MS) / HOUR_MS);
  const minutes = Math.floor((remaining % HOUR_MS) / MINUTE_MS);
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${minutes}m`;
  return `${minutes}m`;
}

/** `resets in 2h 13m`, or null when the window has no reset. */
export function formatResetsIn(window: PlanUsageWindow, now: number): string | null {
  if (window.resets_at_ms == null) return null;
  return window.resets_at_ms <= now
    ? "resets now"
    : `resets in ${formatDuration(window.resets_at_ms - now)}`;
}

/** "just now", "4m ago", "2h ago". */
export function formatCheckedAgo(at: number, now: number): string {
  const ms = now - at;
  if (ms < MINUTE_MS) return "just now";
  return `${formatDuration(ms)} ago`;
}

/** Fill tone by quota LEFT, the inverse of the lane meters' used-share tone. */
function remainingTone(remaining: number): string {
  if (remaining <= 15) return "bg-status-attention";
  if (remaining <= 40) return "bg-accent-ember";
  return "bg-status-open";
}

const PACE: Record<LimitPace, { label: string; icon: typeof Gauge }> = {
  ahead: {
    label: "Ahead of pace: spending faster than the window elapses",
    icon: TrendingUp,
  },
  on: { label: "On pace with the window", icon: Gauge },
  under: {
    label: "Under pace: headroom left for the rest of the window",
    icon: TrendingDown,
  },
};

function PaceIcon({ pace }: { pace: LimitPace }) {
  const Icon = PACE[pace].icon;
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span
          role="img"
          aria-label={PACE[pace].label}
          className={cn(
            "inline-flex",
            pace === "ahead" ? "text-status-attention" : "text-muted-foreground",
          )}
        >
          <Icon className="size-3.5" aria-hidden />
        </span>
      </TooltipTrigger>
      <TooltipContent side="top">{PACE[pace].label}</TooltipContent>
    </Tooltip>
  );
}

/**
 * One window as a full-width bar. The fill is the share of quota still
 * open; the hairline is the share of the window's time still to run, which
 * is where even spending would have left the fill.
 */
function WindowBar({ window, now }: { window: PlanUsageWindow; now: number }) {
  const remaining = remainingPercent(window);
  const elapsed = elapsedShare(window, now);
  const timeLeft = elapsed === null ? null : Math.round((1 - elapsed) * 100);
  const resetsIn = formatResetsIn(window, now);
  const resetsAt =
    window.resets_at_ms != null ? formatResetTime(window.resets_at_ms, now) : null;
  const summary = `${limitWindowLabel(window)}: ${remaining}% left${
    timeLeft === null ? "" : `, ${timeLeft}% of the window left`
  }${resetsIn ? `, ${resetsIn}` : ""}`;

  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <div
          role="img"
          aria-label={summary}
          tabIndex={0}
          className="relative h-5 cursor-default rounded-full outline-none focus-visible:ring-2 focus-visible:ring-ring"
        >
          <div className="absolute inset-x-0 inset-y-[7px] rounded-full bg-muted-foreground/20" />
          {remaining > 0 && (
            <div
              className={cn(
                "absolute inset-y-[7px] left-0 rounded-full",
                remainingTone(remaining),
              )}
              style={{ width: `${remaining}%` }}
            />
          )}
          {timeLeft !== null && (
            <span
              aria-hidden
              className="absolute inset-y-[3px] w-px -translate-x-1/2 bg-foreground/70"
              style={{ left: `${timeLeft}%` }}
            />
          )}
        </div>
      </TooltipTrigger>
      <TooltipContent side="top">
        <span className="flex flex-col gap-0.5">
          <span>
            {remaining}% left
            {timeLeft !== null ? ` · ${timeLeft}% of the window left` : ""}
          </span>
          {timeLeft !== null && (
            <span className="opacity-70">The line is where even spending would be.</span>
          )}
          {resetsAt && (
            <span className="opacity-70">
              Resets {resetsAt}
              {resetsIn ? ` · ${resetsIn.replace("resets ", "")}` : ""}
            </span>
          )}
        </span>
      </TooltipContent>
    </Tooltip>
  );
}

function sortWindows(windows: PlanUsageWindow[]): PlanUsageWindow[] {
  return [...windows].sort(
    (a, b) => KIND_ORDER.indexOf(a.kind) - KIND_ORDER.indexOf(b.kind),
  );
}

function ProviderLimits({
  provider,
  quota,
  status,
  now,
}: {
  provider: string;
  quota: ProviderQuota | undefined;
  status: QuotaProbeStatus | undefined;
  now: number;
}) {
  const windows = sortWindows(quota?.windows ?? []);
  const stale = quota != null && now - quota.received_at_ms > STALE_AFTER_MS;
  // A failed read beside an older reading still shows the reading.
  const notice =
    status && (status.outcome === "failed" || status.outcome === "unavailable")
      ? status
      : undefined;

  return (
    <section className="flex flex-col gap-2.5">
      <div className="flex items-center gap-2.5">
        {isKnownProvider(provider) ? (
          <ProviderLogo provider={provider} className="h-[18px] w-[18px]" />
        ) : (
          <span className="h-[18px] w-[18px]" aria-hidden />
        )}
        <h3 className="text-body font-medium">{seriesLabel(provider)}</h3>
        {quota?.plan_label && (
          <span className="text-label text-muted-foreground">{quota.plan_label}</span>
        )}
        {quota && windows.length > 0 && (
          <span
            className={cn(
              "ml-auto text-caption",
              stale ? "text-status-attention" : "text-muted-foreground/80",
            )}
            title={new Date(quota.received_at_ms).toLocaleString()}
          >
            {stale ? "may be out of date · " : ""}checked{" "}
            {formatCheckedAgo(quota.received_at_ms, now)}
          </span>
        )}
      </div>

      {windows.length > 0 && (
        <div className="grid grid-cols-[minmax(0,9.5rem)_4.75rem_minmax(0,1fr)_1rem_7.5rem] items-center gap-x-4 gap-y-1.5 rounded-lg border border-border/60 bg-muted/30 px-4 py-3">
          {windows.map((window, index) => {
            const pace = paceOf(window, now);
            const resetsIn = formatResetsIn(window, now);
            return (
              // Kind and label alone can repeat: Codex reports both of its
              // windows as label-less "other" when it omits their length.
              <div key={`${window.kind}:${window.label ?? ""}:${index}`} className="contents">
                <span className="truncate text-label text-muted-foreground">
                  {limitWindowLabel(window)}
                </span>
                <span className="select-text text-right font-mono text-body-sm tabular-nums">
                  {remainingPercent(window)}%{" "}
                  <span className="font-sans text-caption text-muted-foreground">left</span>
                </span>
                <WindowBar window={window} now={now} />
                <span className="flex justify-center">
                  {pace && <PaceIcon pace={pace} />}
                </span>
                <span
                  className="truncate text-right text-caption tabular-nums text-muted-foreground"
                  title={
                    window.resets_at_ms != null
                      ? new Date(window.resets_at_ms).toLocaleString()
                      : undefined
                  }
                >
                  {resetsIn ?? ""}
                </span>
              </div>
            );
          })}
        </div>
      )}

      {notice?.message && (
        <p
          className={cn(
            "flex items-center gap-2 rounded-lg border border-border/60 px-4 py-2.5 text-label",
            notice.outcome === "failed"
              ? "text-status-attention"
              : "text-muted-foreground",
          )}
        >
          {notice.outcome === "failed" && (
            <TriangleAlert className="size-3.5 shrink-0" aria-hidden />
          )}
          {notice.outcome === "failed"
            ? `Could not read limits: ${notice.message}`
            : notice.message}
        </p>
      )}
    </section>
  );
}

/**
 * Settings → Usage → Limits: every plan window each provider reports, with
 * how much is left, whether spending is ahead of the clock, and when it
 * resets. Answers "can I keep going" at a glance.
 */
export function UsageLimitsView({
  quota,
  statuses,
  error = null,
  now,
  loading,
}: {
  quota: Record<string, ProviderQuota>;
  statuses: QuotaProbeStatus[];
  /** The read failed as a whole, before any provider answered. */
  error?: string | null;
  now: number;
  /** True until the first direct read settles. */
  loading: boolean;
}) {
  const statusFor = new Map(statuses.map((status) => [status.provider, status]));
  const providers = [
    ...new Set([
      ...Object.keys(quota).filter((p) => (quota[p]?.windows.length ?? 0) > 0),
      ...statuses
        .filter((s) => s.outcome === "failed" || s.outcome === "unavailable")
        .map((s) => s.provider),
    ]),
  ].sort((a, b) => {
    const ra = PROVIDER_ORDER.indexOf(a);
    const rb = PROVIDER_ORDER.indexOf(b);
    if (ra !== rb) return (ra === -1 ? 99 : ra) - (rb === -1 ? 99 : rb);
    return a.localeCompare(b);
  });

  const errorNotice = error ? (
    <p className="flex items-center gap-2 rounded-lg border border-border/60 px-4 py-2.5 text-label text-status-attention">
      <TriangleAlert className="size-3.5 shrink-0" aria-hidden />
      Could not read plan limits: {error}
    </p>
  ) : null;

  if (providers.length === 0) {
    if (errorNotice && !loading) return errorNotice;
    return loading ? (
      <div className="flex items-center gap-2 py-6 text-body text-muted-foreground">
        <Loader2 className="size-4 animate-spin" aria-hidden />
        Reading plan limits…
      </div>
    ) : (
      <div className="rounded-lg border border-border/60 bg-muted/30 px-4 py-10 text-center text-body text-muted-foreground">
        No provider on this machine reports plan limits. Claude Code and Codex
        report them when signed in with a subscription.
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-6">
      {errorNotice}
      {providers.map((provider) => (
        <ProviderLimits
          key={provider}
          provider={provider}
          quota={quota[provider]}
          status={statusFor.get(provider)}
          now={now}
        />
      ))}
      <p className="px-1 text-label leading-relaxed text-muted-foreground">
        Read directly from each provider&apos;s CLI on this machine. Bars show
        quota left; the vertical line marks how much of the window&apos;s time
        is left, where even spending would put the bar.
      </p>
    </div>
  );
}
