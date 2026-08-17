/**
 * What the Timeline rail draws, assembled from three sources.
 *
 * The host serves one of them. The other two are things only Codemux
 * knows:
 *
 * - **The agent runs** are local (see `pr-agent-runs-store`), and they
 *   are the reason this screen exists — a run sits next to the review
 *   comment that prompted it, which is a relationship the host has no
 *   record of.
 * - **The checks row** is not a timeline event at any host. It is the
 *   live checks query, rendered last and always "now", because a check
 *   that is *currently* running is a fact about the present, not a dated
 *   entry in a history. Reading it off the timeline payload instead would
 *   show the state at the last push rather than the state now.
 *
 * Kept as a pure function over its inputs so the merging, the ordering
 * and the filter can be tested without rendering anything.
 */

import type { CheckInfo, PrTimelineEvent, PullRequestInfo } from "@/tauri/types";
import type { AgentRunRecord } from "@/stores/pr-agent-runs-store";
import { checkState, plural } from "@/components/workspace/review/review-ui";

/** "Everything" shows the local runs; "host" shows what a teammate on
 *  the web would see. */
export type TimelineFilter = "everything" | "host";

export interface ChecksSummary {
  passed: number;
  failed: number;
  running: number;
  total: number;
  /** "4 checks passed, 1 running". */
  sentence: string;
  /** Drives the spinner on the rail's last dot. */
  spinning: boolean;
}

export type TimelineEntry =
  | { type: "host"; id: string; at: number | null; event: PrTimelineEvent }
  | { type: "agent"; id: string; at: number; run: AgentRunRecord }
  | { type: "checks"; id: string; at: number; checks: ChecksSummary };

function timeOf(iso: string | null | undefined): number | null {
  if (!iso) return null;
  const ms = Date.parse(iso);
  return Number.isNaN(ms) ? null : ms;
}

/**
 * The checks row, from the live query rather than the timeline payload.
 *
 * Returns null when there are no checks at all: a rail ending in "0
 * checks" says nothing, and the rest of the history is unaffected.
 */
export function summarizeChecks(checks: CheckInfo[]): ChecksSummary | null {
  if (checks.length === 0) return null;
  let passed = 0;
  let failed = 0;
  let running = 0;
  for (const check of checks) {
    const state = checkState(check.conclusion, check.status);
    if (state === "pass") passed++;
    else if (state === "fail") failed++;
    else if (state === "running") running++;
  }

  // Only the non-zero buckets, so a green PR reads "12 checks passed"
  // rather than "12 checks passed, 0 failed, 0 running".
  const parts: string[] = [];
  if (passed > 0) parts.push(`${plural(passed, "check")} passed`);
  if (failed > 0) parts.push(`${failed} failed`);
  if (running > 0) parts.push(`${running} running`);
  // Everything skipped or neutral still deserves a row that says so.
  if (parts.length === 0) parts.push(`${plural(checks.length, "check")} reported`);

  return {
    passed,
    failed,
    running,
    total: checks.length,
    sentence: parts.join(", "),
    spinning: running > 0,
  };
}

export interface BuildTimelineArgs {
  pr: PullRequestInfo;
  /** Host events, from `get_pr_timeline`. */
  events: PrTimelineEvent[];
  /** Local runs for this PR, already selected. */
  runs: AgentRunRecord[];
  checks: CheckInfo[];
  filter: TimelineFilter;
}

/**
 * The rail, oldest first.
 *
 * Ordering rules, in priority order:
 *  1. "Opened" is always first — it is the only event that cannot have
 *     happened after another, and hosts do not serve it.
 *  2. Everything dated sorts by its timestamp; ties keep the order they
 *     arrived in, which for host events is the host's own ordering.
 *  3. Undated host events keep their position relative to the entry
 *     before them rather than being hoisted to the top or dropped.
 *  4. The checks row is always last. It is "now".
 *
 * Because the rail is built oldest-first and new events land at the
 * bottom, a poll appends rather than rearranges — nothing above the
 * pointer moves when an event arrives (binding rule 3).
 */
export function buildTimeline({
  pr,
  events,
  runs,
  checks,
  filter,
}: BuildTimelineArgs): TimelineEntry[] {
  const openedAt = timeOf(pr.created_at);

  // Counted from the history rather than fetched: gh's `commits` field is
  // an array, and this PR is re-fetched every 2.5s. Null when the host
  // served no commit events at all, so the row says "opened this pull
  // request" instead of claiming zero commits.
  const commitCount = events.filter((e) => e.kind === "committed").length || null;

  const opened: TimelineEntry = {
    type: "host",
    id: "opened",
    at: openedAt,
    event: {
      id: "opened",
      actor: pr.author ?? null,
      created_at: pr.created_at ?? null,
      kind: "opened",
      commits: commitCount,
    },
  };

  const hostEntries: TimelineEntry[] = events.map((event) => ({
    type: "host",
    id: event.id,
    at: timeOf(event.created_at),
    event,
  }));

  // "Host only" is what a teammate would see, so the local runs are not
  // hidden by opacity or a badge — they are simply not in the list.
  const agentEntries: TimelineEntry[] =
    filter === "host"
      ? []
      : runs.map((run) => ({
          type: "agent",
          id: run.id,
          at: run.createdAt,
          run,
        }));

  // Undated entries inherit the last known time so they stay put instead
  // of collapsing to the top of the rail.
  let lastKnown = openedAt ?? 0;
  const dated = [...hostEntries, ...agentEntries].map((entry, index) => {
    if (entry.at != null) lastKnown = entry.at;
    return { entry, index, sortAt: entry.at ?? lastKnown };
  });

  dated.sort((a, b) => a.sortAt - b.sortAt || a.index - b.index);

  const out: TimelineEntry[] = [opened, ...dated.map((d) => d.entry)];

  const summary = summarizeChecks(checks);
  if (summary) {
    out.push({ type: "checks", id: "checks", at: Date.now(), checks: summary });
  }
  return out;
}
