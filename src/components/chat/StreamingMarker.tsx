import { useEffect, useMemo, useRef } from "react";

import { AgentOrb } from "@/components/ui/agent-orb";
import { turnOrbActivity } from "@/lib/agent-chat/orb-activity";
import type { ChatViewItem } from "@/lib/agent-chat/types";

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
 */
export function StreamingMarker({ messages }: { messages: ChatViewItem[] }) {
  const label = deriveStreamingLabel(messages);
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
        <span className="shimmer text-[13px] font-semibold">{label}</span>
        {startedAt != null && (
          // Populated imperatively by the effect; render nothing when no
          // turn start is derivable (e.g. hydrated old transcripts) so the
          // marker never shows a bogus number.
          <span
            ref={elapsedRef}
            className="font-mono text-[11px] tabular-nums text-muted-foreground"
            aria-hidden
          />
        )}
      </span>
    </div>
  );
}

/** Status line derived from the transcript tail. Running tool → "Running
 *  <tool>…", streaming prose → "Writing…", streaming reasoning →
 *  "Thinking…", otherwise the neutral "Working…". */
export function deriveStreamingLabel(messages: ChatViewItem[]): string {
  const last = messages[messages.length - 1];
  if (!last) return "Working…";
  switch (last.kind) {
    case "tool_call":
      return last.status === "running" ? `Running ${last.tool_name}…` : "Working…";
    case "assistant_message":
      return last.streaming ? "Writing…" : "Working…";
    case "reasoning":
      return last.streaming ? "Thinking…" : "Working…";
    default:
      return "Working…";
  }
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
