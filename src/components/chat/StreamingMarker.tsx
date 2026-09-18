import { ChevronRight } from "lucide-react";
import { useEffect, useMemo, useRef } from "react";

import { AgentOrb } from "@/components/ui/agent-orb";
import { turnOrbActivity } from "@/lib/agent-chat/orb-activity";
import { countRunningSubagents } from "@/lib/agent-chat/subagents";
import type { ChatViewItem } from "@/lib/agent-chat/types";
import { cn } from "@/lib/utils";
import { useUIStore } from "@/stores/ui-store";

import { formatActivityDuration } from "./activity-steps";

/**
 * Tail "working" marker (design D9) — the last row inside the scroller
 * while a turn is in flight and no approval is pending. The agent orb sits
 * in the 29px turn gutter (aligned under the assistant avatar) next to a
 * shimmering status line, followed by a live elapsed-time suffix (e.g.
 * "Writing… · 40s"). Gating lives in `shouldShowThinkingIndicator`; this
 * component derives the label, the orb's activity, and the turn start.
 *
 * This is the thread's one live orb: it stands for the turn as a whole, so
 * individual tool-call rows above it stay still. There is one orb per live
 * thing.
 *
 * While the agent is waiting on background tasks, the label becomes a
 * button that opens the Subagents panel, which lists those same tasks
 * (subagents and background shell jobs). The line names the work, so it
 * links to it.
 */
export function StreamingMarker({
  messages,
  workspaceId,
}: {
  messages: ChatViewItem[];
  workspaceId?: string | null;
}) {
  const { label, waiting } = useMemo(
    () => deriveStreamingStatus(messages),
    [messages],
  );
  const activity = useMemo(() => turnOrbActivity(messages), [messages]);
  const startedAt = useMemo(() => deriveTurnStartedAt(messages), [messages]);
  const elapsedRef = useRef<HTMLSpanElement>(null);

  // Tick the elapsed suffix once a second WITHOUT re-rendering the
  // transcript: write `textContent` straight to the text node. React never
  // sees the per-second change, so the memoized rows above stay untouched.
  useEffect(() => {
    if (startedAt == null) return;
    const node = elapsedRef.current;
    if (!node) return;
    const render = () => {
      node.textContent = `· ${formatActivityDuration(Date.now() - startedAt)}`;
    };
    render();
    const id = setInterval(render, 1000);
    return () => clearInterval(id);
  }, [startedAt]);

  return (
    <div
      className="flex items-center gap-[13px] pt-0.5"
      role="status"
      aria-label="Agent is working"
    >
      <span className="flex w-[29px] shrink-0 justify-center">
        {/* The row already announces itself via role="status", so the orb
            is decorative here — its per-state label would otherwise be
            read out as a second, competing status. */}
        <AgentOrb size={20} {...activity} aria-hidden />
      </span>
      <span className="flex min-w-0 items-baseline gap-1.5">
        {waiting > 0 && workspaceId ? (
          <WaitingLabel
            label={label}
            waiting={waiting}
            workspaceId={workspaceId}
          />
        ) : (
          <span className="shimmer text-body font-semibold">{label}</span>
        )}
        {startedAt != null && (
          // Populated imperatively by the effect; render nothing when no
          // turn start is derivable (e.g. hydrated old transcripts) so the
          // marker never shows a bogus number.
          <span
            ref={elapsedRef}
            className="font-mono text-label tabular-nums text-muted-foreground"
            aria-hidden
          />
        )}
      </span>
    </div>
  );
}

/** The "Waiting on N background tasks…" label as a link to the Subagents
 *  panel. The shimmer stays on the text so it still reads as live status.
 *  A quiet `View ›` cue beside it matches the agents row in the transcript. */
function WaitingLabel({
  label,
  waiting,
  workspaceId,
}: {
  label: string;
  waiting: number;
  workspaceId: string;
}) {
  const selected = useUIStore(
    (state) => state.rightPanelTabs[workspaceId] === "subagents",
  );
  const setRightPanelTab = useUIStore((state) => state.setRightPanelTab);
  return (
    <button
      type="button"
      onClick={() => setRightPanelTab(workspaceId, "subagents")}
      aria-pressed={selected}
      aria-label={`View ${waiting} background task${waiting === 1 ? "" : "s"}`}
      className={cn(
        "group/waiting -mx-1 flex min-w-0 items-baseline gap-1.5 rounded-md px-1 text-left transition-colors duration-150 hover:bg-surface-1 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/70",
        selected && "bg-surface-2",
      )}
    >
      <span className="shimmer truncate text-body font-semibold">{label}</span>
      <span className="flex shrink-0 items-center gap-0.5 self-center text-caption font-medium text-muted-foreground/70 transition-colors duration-150 group-hover/waiting:text-foreground/80">
        View
        <ChevronRight
          className="size-3 transition-transform duration-150 group-hover/waiting:translate-x-0.5"
          aria-hidden
        />
      </span>
    </button>
  );
}

/** Status line derived from the transcript tail. Running tool → "Running
 *  <tool>…", streaming prose → "Writing…", streaming reasoning →
 *  "Thinking…". A settled tail while background tasks (delegated
 *  subagents or background shell jobs) are still running is the agent
 *  waiting on them → "Waiting on N background tasks…"; otherwise the
 *  neutral "Working…". `waiting` is the number of background tasks being
 *  waited on, and non-zero only for a "Waiting on…" label. */
export function deriveStreamingStatus(messages: ChatViewItem[]): {
  label: string;
  waiting: number;
} {
  const last = messages[messages.length - 1];
  if (!last) return { label: "Working…", waiting: 0 };
  switch (last.kind) {
    case "tool_call":
      if (last.status === "running") {
        return { label: `Running ${last.tool_name}…`, waiting: 0 };
      }
      break;
    case "assistant_message":
      if (last.streaming) return { label: "Writing…", waiting: 0 };
      break;
    case "reasoning":
      if (last.streaming) return { label: "Thinking…", waiting: 0 };
      break;
    default:
      break;
  }
  const waiting = countRunningSubagents(messages, true);
  if (waiting === 1) return { label: "Waiting on a background task…", waiting };
  if (waiting > 1) {
    return { label: `Waiting on ${waiting} background tasks…`, waiting };
  }
  return { label: "Working…", waiting: 0 };
}

/**
 * Best-effort wall-clock start of the active turn, for the elapsed-time
 * suffix. Derived purely from timestamps the reducer already stamps (via
 * its injectable `Clock`) — no new reducer state:
 *
 *  1. Find the active turn's prompt: the last NON-queued `user_message`
 *     (queued follow-ups sit at the very bottom by seq but belong to a
 *     later turn).
 *  2. Return the earliest `started_at` among the reasoning / tool_call
 *     steps of that turn (items at or after the prompt's seq).
 *
 * Returns `null` when nothing after the prompt carries a `started_at`
 * (e.g. the gap right after send before the first step, or a hydrated
 * transcript whose rows predate the timestamp fields) — the caller then
 * renders no suffix rather than a fabricated duration.
 */
export function deriveTurnStartedAt(messages: ChatViewItem[]): number | null {
  let promptSeq = -Infinity;
  for (const m of messages) {
    if (m.kind === "user_message" && !m.queued && m.seq >= promptSeq) {
      promptSeq = m.seq;
    }
  }
  let earliest: number | null = null;
  for (const m of messages) {
    if (m.seq < promptSeq) continue;
    const startedAt =
      m.kind === "reasoning" || m.kind === "tool_call" ? m.started_at : undefined;
    if (startedAt != null && (earliest == null || startedAt < earliest)) {
      earliest = startedAt;
    }
  }
  return earliest;
}
