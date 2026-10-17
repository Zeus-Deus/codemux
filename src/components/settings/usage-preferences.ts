import type { UsagePeriod } from "@/tauri/commands";

/** Which page the Usage section shows: spend history or plan limits. */
export type UsageView = "activity" | "limits";
export type UsageMetric = "cost" | "tokens";
/** How "Over time" shows the period: stacked columns or a day table. */
export type UsageTimeView = "chart" | "table";

export interface UsagePreferences {
  view: UsageView;
  period: UsagePeriod;
  metric: UsageMetric;
  timeView: UsageTimeView;
  /** Provider ids filtered out of the page. Stored as hidden rather than
   *  shown, so a provider that starts reporting later appears by default. */
  hiddenProviders: string[];
}

const STORAGE_KEY = "codemux.usage.preferences.v1";

export const DEFAULT_USAGE_PREFERENCES: UsagePreferences = {
  view: "activity",
  period: "7d",
  metric: "cost",
  timeView: "chart",
  hiddenProviders: [],
};

const VIEWS: readonly UsageView[] = ["activity", "limits"];
const PERIODS: readonly UsagePeriod[] = ["today", "7d", "30d", "90d"];
const METRICS: readonly UsageMetric[] = ["cost", "tokens"];
const TIME_VIEWS: readonly UsageTimeView[] = ["chart", "table"];

function pick<T extends string>(value: unknown, allowed: readonly T[], fallback: T): T {
  return allowed.includes(value as T) ? (value as T) : fallback;
}

/** The last choices made on this device, field by field, so one stale or
 *  unknown value cannot reset the rest. */
export function readUsagePreferences(): UsagePreferences {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return DEFAULT_USAGE_PREFERENCES;
    const parsed = JSON.parse(raw) as Record<string, unknown>;
    const d = DEFAULT_USAGE_PREFERENCES;
    return {
      view: pick(parsed.view, VIEWS, d.view),
      period: pick(parsed.period, PERIODS, d.period),
      metric: pick(parsed.metric, METRICS, d.metric),
      timeView: pick(parsed.timeView, TIME_VIEWS, d.timeView),
      hiddenProviders: Array.isArray(parsed.hiddenProviders)
        ? parsed.hiddenProviders.filter((p): p is string => typeof p === "string")
        : d.hiddenProviders,
    };
  } catch {
    return DEFAULT_USAGE_PREFERENCES;
  }
}

export function saveUsagePreferences(preferences: UsagePreferences): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(preferences));
  } catch {
    // Storage full or disabled: the choice still holds for this visit.
  }
}
