import { Check, ChevronDown, ChevronRight, LoaderCircle } from "lucide-react";
import { memo, useEffect, useId, useRef, useState } from "react";

import {
  formatElapsed,
  runningSubagentEntries,
  subagentActivityLine,
  subagentElapsedMs,
  type RunningSubagentEntry,
} from "@/lib/agent-chat/subagents";
import type { ChatViewItem } from "@/lib/agent-chat/types";
import { cn } from "@/lib/utils";

import { CHAT_COLUMN } from "./chat-column";

/** How long the green "just finished" flash stays up before the bar
 *  disappears entirely (design "for ~2.5s, then disappears"). */
const FINISHED_FLASH_MS = 2500;

/**
 * Docked live subagent activity bar (design "A living status, docked by
 * the composer"). One bar for the whole thread: counts every live
 * (running|pending) subagent across every `subagent_run` card, no matter
 * which reply spawned it. Mounted between the transcript and the
 * composer; renders nothing while idle — no resting state, and this
 * replaces the old pane-header / title-bar "N subagents running" pills
 * (design note: "no tab-strip pill... looks like a broken tab").
 *
 * - 1 running -> action chip is "View"; clicking the bar jumps straight
 *   to that subagent's card.
 * - >1 running -> action chip is "Show all" / "Hide"; clicking the bar
 *   toggles an expand list (opens upward) with one row per running
 *   subagent, each jumping to its own card.
 * - Just finished (running count observed transitioning >0 -> 0): the
 *   bar flashes green for {@link FINISHED_FLASH_MS}, then unmounts.
 *   The flash itself is clickable (design gallery "Jump" CTA) and jumps
 *   to the card of the last subagent that was still running.
 *
 * Self-contained: owns its own open/finished-flash state and the 1s
 * elapsed-time tick. The actual transcript scroll + highlight is done by
 * the caller (`onJump`) since that requires DOM access scoped to the
 * owning pane (see `AgentChatPane.tsx`'s jump handler).
 */
export const SubagentActivityBar = memo(function SubagentActivityBar({
  messages,
  threadId,
  onJump,
}: {
  messages: ChatViewItem[];
  threadId: string | null;
  onJump: (cardId: string) => void;
}) {
  const entries = runningSubagentEntries(messages);
  const count = entries.length;
  const now = useNow(count > 0);

  const listId = useId();
  const [open, setOpen] = useState(false);
  const [finishedFlash, setFinishedFlash] = useState(false);
  const prevCountRef = useRef(count);
  const prevThreadRef = useRef(threadId);
  // Card of the most recent still-running subagent, captured every render
  // while anything runs — by the time the >0 → 0 transition is observed,
  // `entries` is already empty, so this ref is what the finished flash's
  // "jump" click targets (the design gallery gives the finished state a
  // Jump CTA).
  const lastRunningCardIdRef = useRef<string | null>(null);
  if (count > 0) {
    lastRunningCardIdRef.current = entries[entries.length - 1].cardId;
  }

  useEffect(() => {
    // A thread switch is not an "observed transition" — reset silently so
    // hydrating into a thread that happens to have zero running subagents
    // never flashes green (design: "No flash on initial mount/thread
    // hydrate, only on an observed transition").
    if (threadId !== prevThreadRef.current) {
      prevThreadRef.current = threadId;
      prevCountRef.current = count;
      lastRunningCardIdRef.current = null;
      setFinishedFlash(false);
      setOpen(false);
      return;
    }

    const wasRunning = prevCountRef.current > 0;
    prevCountRef.current = count;

    if (wasRunning && count === 0) {
      setFinishedFlash(true);
      const timer = window.setTimeout(() => {
        setFinishedFlash(false);
      }, FINISHED_FLASH_MS);
      return () => window.clearTimeout(timer);
    }

    // The expand list only exists in the multi (>1) state. Reset `open`
    // whenever the count falls out of it — otherwise a stale `open=true`
    // (list open → one finishes → another starts) would pop the list back
    // open with no click.
    if (count <= 1) setOpen(false);
    return undefined;
    // eslint-disable-next-line react-hooks/exhaustive-deps -- deliberately
    // keyed only on count/threadId; prevCountRef/prevThreadRef are refs.
  }, [count, threadId]);

  if (count === 0 && !finishedFlash) return null;

  if (count === 0 && finishedFlash) {
    const finishedCardId = lastRunningCardIdRef.current;
    const jumpToFinished = () => {
      if (finishedCardId) onJump(finishedCardId);
    };
    return (
      <div className={CHAT_COLUMN}>
        <div
          data-testid="subagent-activity-bar"
          data-tone="finished"
          role="button"
          tabIndex={0}
          onClick={jumpToFinished}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              jumpToFinished();
            }
          }}
          title="Jump to the Subagents card"
          className="flex h-11 cursor-pointer items-center gap-2.5 rounded-xl border border-status-open/30 bg-status-open/10 px-3.5 hover:bg-status-open/[0.15]"
        >
          <span className="flex h-5 w-5 shrink-0 items-center justify-center">
            <Check
              className="h-[19px] w-[19px] text-status-open"
              strokeWidth={1.8}
              aria-hidden
            />
          </span>
          <span className="shrink-0 whitespace-nowrap text-[13px] font-bold text-foreground">
            Subagents finished
          </span>
          <span className="min-w-0 flex-1 truncate font-mono text-[12px] text-muted-foreground">
            all tasks complete · results are in the thread
          </span>
        </div>
      </div>
    );
  }

  const multi = count > 1;
  const primary = entries[0];
  const primaryName =
    primary.subagent.name ?? primary.subagent.agentType ?? "Subagent";
  const primaryActivity = subagentActivityLine(primary.subagent);
  const primaryElapsedMs = subagentElapsedMs(primary.subagent, now);

  const handleBarClick = () => {
    if (multi) {
      setOpen((cur) => !cur);
    } else {
      onJump(primary.cardId);
    }
  };
  const handleBarKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      handleBarClick();
    }
  };
  const handleRowJump = (entry: RunningSubagentEntry) => {
    setOpen(false);
    onJump(entry.cardId);
  };

  return (
    <div className={CHAT_COLUMN}>
      {multi && open && (
        <div
          id={listId}
          data-testid="subagent-activity-bar-list"
          className="rise-in mb-2 overflow-hidden rounded-xl border border-border bg-popover shadow-lg"
        >
          <div className="flex items-center gap-2 border-b border-border/60 px-3 py-2.5">
            <span className="whitespace-nowrap text-[12px] font-bold text-foreground">
              {count} subagents running
            </span>
            <span className="text-[11px] text-muted-foreground">
              across this thread · tap one to jump
            </span>
          </div>
          <div className="p-1.5">
            {entries.map((entry) => (
              <SubagentActivityBarRow
                key={entry.subagent.id}
                entry={entry}
                now={now}
                onJump={() => handleRowJump(entry)}
              />
            ))}
          </div>
        </div>
      )}

      <div
        data-testid="subagent-activity-bar"
        data-tone="running"
        role="button"
        tabIndex={0}
        aria-expanded={multi ? open : undefined}
        aria-controls={multi && open ? listId : undefined}
        onClick={handleBarClick}
        onKeyDown={handleBarKeyDown}
        title={
          multi ? "Show all running subagents" : "Jump to the Subagents card"
        }
        className="relative flex h-11 cursor-pointer items-center gap-2.5 overflow-hidden rounded-xl border border-status-working/30 bg-status-working/10 py-0 pl-3.5 pr-1.5 hover:bg-status-working/[0.15]"
      >
        <div className="absolute inset-x-0 top-0 h-0.5 overflow-hidden bg-status-working/15">
          {/* 38% wide — matches the `cm-sweep` keyframe defaults, so no
              travel overrides are needed here. */}
          <div className="cm-sweep absolute top-0 left-0 h-0.5 w-[38%] rounded-full bg-status-working" />
        </div>
        <span className="flex h-5 w-5 shrink-0 items-center justify-center">
          <LoaderCircle
            className="h-[17px] w-[17px] animate-spin text-status-working"
            strokeWidth={1.9}
            aria-hidden
          />
        </span>
        <span className="shrink-0 whitespace-nowrap text-[13px] font-bold text-foreground">
          {count} subagent{count === 1 ? "" : "s"} running
        </span>
        <span
          className="h-[3px] w-[3px] shrink-0 rounded-full bg-muted-foreground"
          aria-hidden
        />
        <span className="shimmer min-w-0 flex-1 truncate font-mono text-[12px]">
          {primaryName} · {primaryActivity}
        </span>
        <span className="shrink-0 whitespace-nowrap font-mono text-[11px] text-muted-foreground">
          {primaryElapsedMs != null ? formatElapsed(primaryElapsedMs) : ""}
        </span>
        <span className="flex h-[30px] shrink-0 items-center gap-1.5 rounded-lg bg-foreground/[0.08] px-2.5 text-[11px] font-semibold text-muted-foreground">
          {multi ? (open ? "Hide" : "Show all") : "View"}
          {multi ? (
            <ChevronDown
              className={cn(
                "h-3 w-3 transition-transform",
                open && "rotate-180",
              )}
              strokeWidth={1.8}
              aria-hidden
            />
          ) : (
            <ChevronRight className="h-3 w-3" strokeWidth={1.8} aria-hidden />
          )}
        </span>
      </div>
    </div>
  );
});

function SubagentActivityBarRow({
  entry,
  now,
  onJump,
}: {
  entry: RunningSubagentEntry;
  now: number;
  onJump: () => void;
}) {
  const { subagent, fromLabel } = entry;
  const name = subagent.name ?? subagent.agentType ?? "Subagent";
  const activity = subagentActivityLine(subagent);
  const elapsedMs = subagentElapsedMs(subagent, now);

  return (
    <div
      data-testid="subagent-activity-bar-row"
      data-subagent-id={subagent.id}
      role="button"
      tabIndex={0}
      onClick={onJump}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onJump();
        }
      }}
      className="flex cursor-pointer items-center gap-2.5 rounded-lg px-2.5 py-2 hover:bg-foreground/[0.06]"
    >
      <span className="flex h-[18px] w-[18px] shrink-0 items-center justify-center">
        <LoaderCircle
          className="h-[15px] w-[15px] animate-spin text-status-working"
          strokeWidth={1.9}
          aria-hidden
        />
      </span>
      <span className="w-20 shrink-0 truncate text-[12px] font-bold text-foreground">
        {name}
      </span>
      <span className="shimmer min-w-0 flex-1 truncate font-mono text-[11px]">
        {activity}
      </span>
      <span className="shrink-0 font-mono text-[10px] text-muted-foreground">
        {elapsedMs != null ? formatElapsed(elapsedMs) : ""}
      </span>
      {fromLabel && (
        <span className="shrink-0 text-[10px] font-semibold text-muted-foreground">
          from {fromLabel}
        </span>
      )}
      <ChevronRight
        className="h-3 w-3 shrink-0 text-muted-foreground"
        strokeWidth={1.7}
        aria-hidden
      />
    </div>
  );
}

/** 1s tick while `active`, so derived elapsed times advance without a
 *  provider `duration_ms`. Frozen (no interval) when nothing is running.
 *  Mirrors `SubagentsCard.tsx`'s local `useNow`. */
function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [active]);
  return now;
}
