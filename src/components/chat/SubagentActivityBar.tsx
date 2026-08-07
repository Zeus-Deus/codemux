import { Check, ChevronDown, ChevronRight } from "lucide-react";
import { memo, useEffect, useId, useRef, useState } from "react";

import { AgentOrb } from "@/components/ui/agent-orb";
import { subagentOrbActivity } from "@/lib/agent-chat/orb-activity";
import {
  formatElapsed,
  runningSubagentEntries,
  subagentActivityLine,
  subagentElapsedMs,
  type RunningSubagentEntry,
} from "@/lib/agent-chat/subagents";
import type { ChatViewItem, SubagentView } from "@/lib/agent-chat/types";
import { resolveOrbState } from "@/lib/orb-state";
import { cn } from "@/lib/utils";

import { TickingText } from "./TickingText";

/** How long the green "just finished" flash stays up before the bar
 *  disappears entirely (design "for ~2.5s, then disappears"). */
const FINISHED_FLASH_MS = 2500;

/** Sweep geometry (design: a 1px, ~30%-wide light travelling across the
 *  strip's top edge from -60% to 260% of the track).
 *
 *  `cm-sweep` resolves its `translateX` percentages against the SEGMENT's
 *  own width, so the track endpoints are converted here: own-width % =
 *  track % ÷ segment %. Reduced motion is handled by the keyframe class
 *  itself, which drops the animation entirely. */
const SWEEP_STYLE = {
  "--cm-sweep-from": "-200%",
  "--cm-sweep-to": "866%",
  animationDuration: "2.4s",
} as React.CSSProperties;

/**
 * The composer's running strip (design 1b, "COMPOSER · RUNNING STRIP").
 * One strip for the whole thread: counts every live (running|pending)
 * subagent across every `subagent_run` card, no matter which reply
 * spawned it.
 *
 * It is **welded inside the composer's top edge** rather than docked as a
 * separate bar above it — the composer keeps one border and one radius,
 * and the strip is a 32px band with a hairline bottom border and a faint
 * foreground tint. There is no saturated filled bar: liveness is carried
 * by the orb plus a 1px accent light sweeping the top edge. Renders
 * nothing while idle — no resting state.
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
 *
 * Mounted through `Composer`'s `topStripSlot`, so the corner radius here
 * is the composer card's 20px minus its 1px border.
 */
export const SubagentActivityBar = memo(function SubagentActivityBar({
  messages,
  threadId,
  streaming,
  onJump,
}: {
  messages: ChatViewItem[];
  threadId: string | null;
  /** The thread's live-run flag. With the run over, provider background
   *  tasks (a background shell command that legitimately outlives the
   *  turn) stop counting as live activity, so the bar can't spin forever
   *  after the turn settled. */
  streaming: boolean;
  onJump: (cardId: string) => void;
}) {
  const entries = runningSubagentEntries(messages, streaming);
  const count = entries.length;

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
        className="flex h-8 cursor-pointer items-center gap-2.5 rounded-t-[19px] border-b border-border/60 bg-status-open/[0.06] pr-2.5 pl-3 hover:bg-status-open/10"
      >
        <span className="flex h-5 w-5 shrink-0 items-center justify-center">
          <Check
            className="h-4 w-4 text-status-open"
            strokeWidth={1.8}
            aria-hidden
          />
        </span>
        <span className="shrink-0 whitespace-nowrap text-[12px] font-semibold text-foreground/80">
          Subagents finished
        </span>
        <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-muted-foreground">
          all tasks complete · results are in the thread
        </span>
      </div>
    );
  }

  const multi = count > 1;
  const primary = entries[0];

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
    <div className="relative">
      {/* The strip sits at the very top of the composer, so its expand
          list opens upward as an overlay (the same `bottom-full` pattern
          the composer's own popups use) instead of pushing layout. */}
      {multi && open && (
        <div
          id={listId}
          data-testid="subagent-activity-bar-list"
          className="rise-in absolute right-0 bottom-full left-0 z-50 mb-2 overflow-hidden rounded-xl border border-border bg-popover shadow-lg"
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
        className="relative flex h-8 cursor-pointer items-center gap-2.5 overflow-hidden rounded-t-[19px] border-b border-border/60 bg-foreground/[0.03] pr-2.5 pl-3 hover:bg-foreground/[0.05]"
      >
        {/* The one moving mark besides the orb: a 1px accent light
            travelling the top edge. No filled progress bar — a saturated
            band across the composer is exactly the noise this redesign
            removes. */}
        <span
          className="cm-sweep pointer-events-none absolute top-0 left-0 h-px w-[30%] bg-gradient-to-r from-transparent via-accent-ember to-transparent"
          style={SWEEP_STYLE}
          aria-hidden
        />
        <span className="flex h-5 w-5 shrink-0 items-center justify-center">
          {/* This orb stands for the whole run, not for any one subagent,
              so it stays on the neutral working state even when a single
              subagent is doing something more specific. Each expanded row
              below owns its own activity-matched orb. */}
          <AgentOrb size={20} aria-hidden />
        </span>
        <span className="shrink-0 whitespace-nowrap text-[12px] font-semibold text-foreground/80">
          {count} subagent{count === 1 ? "" : "s"} running
        </span>
        <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-muted-foreground">
          {runningActivityLabel(entries)}
        </span>
        <TickingText
          className="shrink-0 whitespace-nowrap font-mono text-[11px] text-muted-foreground"
          compute={(now) => elapsedLabel(primary.subagent, now)}
        />
        <span className="flex shrink-0 items-center gap-1 text-[11px] font-semibold text-foreground/80">
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

/**
 * The strip's mono activity label — what kind of busy the run currently
 * is, e.g. "solving · connecting".
 *
 * Reuses the orb-state mapper rather than inventing a second vocabulary:
 * the strip then names the same states the rows are animating. Deduped
 * (three parallel greps read as one "searching") and capped, because this
 * label shares a 32px line with the count, the elapsed time and the
 * action.
 *
 * Deliberately NOT gated on the Settings "Match the orb to the activity"
 * pin. That setting governs whether the *animation* varies; the app knows
 * what the tools are either way, and a text label that says less to match
 * a calmer orb would just be withholding.
 */
function runningActivityLabel(entries: RunningSubagentEntry[]): string {
  const states = new Set<string>();
  for (const entry of entries) {
    states.add(resolveOrbState(subagentOrbActivity(entry.subagent)));
    if (states.size >= 3) break;
  }
  return [...states].join(" · ");
}

function SubagentActivityBarRow({
  entry,
  onJump,
}: {
  entry: RunningSubagentEntry;
  onJump: () => void;
}) {
  const { subagent, fromLabel } = entry;
  const name = subagent.name ?? subagent.agentType ?? "Subagent";
  const activity = subagentActivityLine(subagent);

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
      <span className="flex h-5 w-5 shrink-0 items-center justify-center">
        <AgentOrb size={20} {...subagentOrbActivity(subagent)} aria-hidden />
      </span>
      <span className="w-20 shrink-0 truncate text-[12px] font-bold text-foreground">
        {name}
      </span>
      <span className="shimmer min-w-0 flex-1 truncate font-mono text-[11px]">
        {activity}
      </span>
      <TickingText
        className="shrink-0 font-mono text-[10px] text-muted-foreground"
        compute={(now) => elapsedLabel(subagent, now)}
      />
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

/** Elapsed suffix for a running subagent; empty when nothing is derivable
 *  (so the bar renders no number rather than a fabricated one). */
function elapsedLabel(subagent: SubagentView, now: number): string {
  const ms = subagentElapsedMs(subagent, now);
  return ms != null ? formatElapsed(ms) : "";
}
