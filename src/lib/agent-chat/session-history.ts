import type { AgentChatSessionRecord } from "@/tauri/commands";

export type SessionBucketKey =
  | "today"
  | "yesterday"
  | "last7"
  | "last30"
  | "older";

export interface SessionBucket {
  key: SessionBucketKey;
  label: string;
  sessions: AgentChatSessionRecord[];
}

const BUCKET_LABELS: Record<SessionBucketKey, string> = {
  today: "Today",
  yesterday: "Yesterday",
  last7: "Last 7 days",
  last30: "Last 30 days",
  older: "Older",
};

// SQLite stores timestamps in UTC (`datetime('now')`), so we bucket
// by UTC-day boundaries. This also makes the grouping deterministic
// regardless of the machine's local timezone — important for tests
// and consistent behaviour across a user's devices.
function startOfUtcDay(date: Date): number {
  return Date.UTC(
    date.getUTCFullYear(),
    date.getUTCMonth(),
    date.getUTCDate(),
  );
}

function bucketForTimestamp(
  timestampMs: number,
  nowMs: number,
): SessionBucketKey {
  const startToday = startOfUtcDay(new Date(nowMs));
  const dayMs = 24 * 60 * 60 * 1000;
  if (timestampMs >= startToday) return "today";
  if (timestampMs >= startToday - dayMs) return "yesterday";
  if (timestampMs >= startToday - 7 * dayMs) return "last7";
  if (timestampMs >= startToday - 30 * dayMs) return "last30";
  return "older";
}

/**
 * SQLite stores timestamps in the UTC `YYYY-MM-DD HH:MM:SS` format
 * produced by `datetime('now')`. Convert to epoch ms; returns NaN for
 * malformed input so the caller can treat those sessions as "older"
 * rather than crashing.
 */
export function parseSqliteTimestamp(value: string): number {
  // Treat the raw timestamp as UTC by appending 'Z'.
  const iso = value.includes("T") ? value : value.replace(" ", "T") + "Z";
  return new Date(iso).getTime();
}

/**
 * Group a flat list of session records into date buckets suitable
 * for the header dropdown. Expects the input to be pre-ordered
 * `last_active_at DESC`; empty buckets are omitted.
 */
export function groupSessionsByDate(
  sessions: AgentChatSessionRecord[],
  nowMs: number = Date.now(),
): SessionBucket[] {
  const bucketMap: Record<SessionBucketKey, AgentChatSessionRecord[]> = {
    today: [],
    yesterday: [],
    last7: [],
    last30: [],
    older: [],
  };
  for (const session of sessions) {
    const ts = parseSqliteTimestamp(session.last_active_at);
    const bucket = Number.isFinite(ts)
      ? bucketForTimestamp(ts, nowMs)
      : "older";
    bucketMap[bucket].push(session);
  }
  const order: SessionBucketKey[] = [
    "today",
    "yesterday",
    "last7",
    "last30",
    "older",
  ];
  return order
    .map((key) => ({
      key,
      label: BUCKET_LABELS[key],
      sessions: bucketMap[key],
    }))
    .filter((bucket) => bucket.sessions.length > 0);
}

/** Human-facing label for a session row. Falls back to a short
 *  suffix of the thread id when the auto-title hasn't landed yet
 *  (first user turn hasn't happened). */
export function sessionDisplayTitle(session: AgentChatSessionRecord): string {
  if (session.title && session.title.trim().length > 0) {
    return session.title;
  }
  const suffix = session.thread_id.slice(-6);
  return `Chat ${suffix}`;
}
