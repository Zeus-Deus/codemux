import { formatClockTime } from "./goal";
import type { UsageLimitState } from "./types";

/**
 * Presentation helpers for a provider usage-limit stop: the transcript
 * record, the composer-strip countdown, and the automatic resume turn.
 */

/** Exact prefix the backend stamps on the turn it sends when it resumes a
 *  usage-limited run on its own. */
export const AUTO_RESUME_PREFIX =
  "[Resumed automatically after a provider usage limit reset";

/** How long "Resuming…" waits for the backend before offering Resume. The
 *  backend fires within ~15s of the resume time. */
export const RESUME_GRACE_MS = 2 * 60_000;

/** Countdowns switch to a per-second tick inside this window. */
export const FINE_TICK_WINDOW_MS = 2 * 60_000;

export const FINE_TICK_MS = 1_000;
export const COARSE_TICK_MS = 15_000;

const WINDOW_LABELS: Record<string, string> = {
  five_hour: "5-hour limit",
  seven_day: "weekly limit",
  seven_day_opus: "weekly Opus limit",
  seven_day_sonnet: "weekly Sonnet limit",
  overage: "overage limit",
};

/** Friendly name for the exhausted window, or `null` when unknown. */
export function usageWindowLabel(window: string | null | undefined): string | null {
  if (!window) return null;
  return WINDOW_LABELS[window] ?? null;
}

/** Whether a user turn is the backend's automatic usage-limit resume. */
export function isAutoResumeText(text: string): boolean {
  return text.startsWith(AUTO_RESUME_PREFIX);
}

/**
 * Viewer-local wall-clock time of a reset: `23:40` today, `Sat 09:00`
 * within the coming week, `Sep 25, 09:00` beyond that.
 */
export function formatResetTime(ms: number, now: number = Date.now()): string {
  const time = formatClockTime(ms);
  const at = new Date(ms);
  const today = new Date(now);
  if (at.toDateString() === today.toDateString()) return time;
  const days = (startOfDay(at) - startOfDay(today)) / 86_400_000;
  if (days > 0 && days < 7) {
    return `${at.toLocaleDateString([], { weekday: "short" })} ${time}`;
  }
  return `${at.toLocaleDateString([], { month: "short", day: "numeric" })}, ${time}`;
}

function startOfDay(d: Date): number {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}

/**
 * Time left until a reset or resume: `45s` and `1m 05s` inside the last two
 * minutes, then whole minutes rounded up (`12m`, `1h 12m`, `2d 3h`) so the
 * label never reads zero while time remains.
 */
export function formatCountdown(ms: number): string {
  const seconds = Math.max(0, Math.ceil(ms / 1000));
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 120) return `1m ${String(seconds - 60).padStart(2, "0")}s`;
  const minutes = Math.ceil(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    const rest = minutes % 60;
    return rest ? `${hours}h ${rest}m` : `${hours}h`;
  }
  const days = Math.floor(hours / 24);
  const rest = hours % 24;
  return rest ? `${days}d ${rest}h` : `${days}d`;
}

/** The transcript record: "Usage limit reached · 5-hour limit · resets 23:40". */
export function usageLimitRecordText(
  resetsAtMs: number | null,
  window: string | null,
  now: number = Date.now(),
): string {
  const parts = ["Usage limit reached"];
  const label = usageWindowLabel(window);
  if (label) parts.push(label);
  if (resetsAtMs !== null) parts.push(`resets ${formatResetTime(resetsAtMs, now)}`);
  return parts.join(" · ");
}

/**
 * Where the composer row stands at `now`:
 * - `armed`: an automatic resume is scheduled in the future.
 * - `resuming`: the resume time passed; the backend is about to send.
 * - `waiting`: nothing armed, but the limit lifts at a known future time.
 * - `reset`: the reset (or a resume that never came) is behind us.
 * - `reached`: nothing armed and no reset time reported.
 */
export type UsageLimitPhase =
  | { kind: "armed"; at: number }
  | { kind: "resuming" }
  | { kind: "waiting"; at: number }
  | { kind: "reset" }
  | { kind: "reached" };

export function usageLimitPhase(
  limit: UsageLimitState,
  now: number,
): UsageLimitPhase {
  const { autoResumeAtMs, resetsAtMs } = limit;
  if (autoResumeAtMs !== null) {
    if (now < autoResumeAtMs) return { kind: "armed", at: autoResumeAtMs };
    if (now < autoResumeAtMs + RESUME_GRACE_MS) return { kind: "resuming" };
    return { kind: "reset" };
  }
  if (resetsAtMs === null) return { kind: "reached" };
  return now < resetsAtMs ? { kind: "waiting", at: resetsAtMs } : { kind: "reset" };
}

/**
 * The next instant the row must re-render on its own: a phase change, or
 * a countdown entering its per-second window. `null` when nothing more
 * changes without a new event.
 */
export function nextUsageLimitBoundary(
  limit: UsageLimitState,
  now: number,
): number | null {
  const candidates: number[] = [];
  const { autoResumeAtMs, resetsAtMs } = limit;
  if (autoResumeAtMs !== null) {
    candidates.push(
      autoResumeAtMs - FINE_TICK_WINDOW_MS,
      autoResumeAtMs,
      autoResumeAtMs + RESUME_GRACE_MS,
    );
  } else if (resetsAtMs !== null) {
    candidates.push(resetsAtMs - FINE_TICK_WINDOW_MS, resetsAtMs);
  }
  const future = candidates.filter((t) => t > now);
  return future.length > 0 ? Math.min(...future) : null;
}

/** Tick cadence for a countdown to `at`. */
export function countdownIntervalMs(at: number, now: number): number {
  return at - now <= FINE_TICK_WINDOW_MS ? FINE_TICK_MS : COARSE_TICK_MS;
}
