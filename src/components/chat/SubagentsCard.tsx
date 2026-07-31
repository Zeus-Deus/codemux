import {
  Ban,
  Check,
  ChevronDown,
  ChevronRight,
  Info,
  LoaderCircle,
  X,
} from "lucide-react";
import { memo, useEffect, useState } from "react";

import {
  describeToolCall,
  isRunning,
  recentToolCalls,
  statusTone,
  subagentActivityLine,
  subagentMetaLine,
} from "@/lib/agent-chat/subagents";
import type { SubagentRunItem, SubagentView } from "@/lib/agent-chat/types";
import { cn } from "@/lib/utils";

/**
 * Subagents orchestration card (design "Subagents, made visible"). One
 * card per contiguous spawn group; one row per subagent with a status
 * spinner/check/x, name + model column, a shimmering live activity line
 * (or muted result line when done), mono meta ("2m 41s · 28 tools"), an
 * Enter button and a chevron that opens an inline "recent activity" peek.
 *
 * Rendered as a single memoized transcript row: a streaming child event
 * mutates only this card's `item` reference, so exactly this row
 * re-renders (issue #77 scroller contract preserved). A local 1s tick
 * refreshes the derived elapsed time while any subagent is running.
 *
 * The root carries `data-subagent-card={item.id}` so the docked
 * {@link SubagentActivityBar} can locate + scroll to + flash-highlight
 * this card by plain DOM query, without any new prop plumbing through
 * MessageList/ChatTranscript.
 */
export const SubagentsCard = memo(function SubagentsCard({
  item,
  onEnter,
}: {
  item: SubagentRunItem;
  onEnter: (subagentId: string) => void;
}) {
  const subs = item.subagents;
  const anyRunning = subs.some((s) => s.status === "running");
  const anyFailed = subs.some((s) => s.status === "failed");
  const now = useNow(anyRunning);
  const [openId, setOpenId] = useState<string | null>(null);

  const doneCount = subs.filter(
    (s) =>
      s.status === "completed" ||
      s.status === "failed" ||
      s.status === "stopped" ||
      s.status === "interrupted",
  ).length;
  const activeCount = subs.filter((s) => isRunning(s)).length;

  return (
    <div
      data-subagent-card={item.id}
      className="overflow-hidden rounded-[13px] border border-border bg-foreground/[0.025]"
    >
      {/* Aggregate header */}
      <div className="flex items-center gap-2.5 border-b border-border/60 px-3.5 py-3">
        <span className="flex h-5 w-5 shrink-0 items-center justify-center">
          {anyRunning ? (
            <LoaderCircle
              className="h-[18px] w-[18px] animate-spin text-status-working"
              strokeWidth={1.8}
              aria-hidden
            />
          ) : anyFailed ? (
            <X
              className="h-[18px] w-[18px] text-status-attention"
              strokeWidth={1.8}
              aria-hidden
            />
          ) : (
            <Check
              className="h-[18px] w-[18px] text-status-open"
              strokeWidth={1.8}
              aria-hidden
            />
          )}
        </span>
        <span className="text-[13px] font-bold text-foreground">Subagents</span>
        <span className="text-[12px] text-muted-foreground">
          {subs.length} {subs.length === 1 ? "task" : "tasks"}
          {anyRunning ? " · running in parallel" : " · complete"}
        </span>
        <span className="ml-auto font-mono text-[11px] text-muted-foreground">
          {doneCount} done · {activeCount} active
        </span>
      </div>

      {/* Rows */}
      <div className="p-1.5">
        {subs.map((sub) => (
          <SubagentRow
            key={sub.id}
            sub={sub}
            now={now}
            open={openId === sub.id}
            onToggle={() =>
              setOpenId((cur) => (cur === sub.id ? null : sub.id))
            }
            onEnter={() => onEnter(sub.id)}
          />
        ))}
      </div>

      {/* Footer note */}
      <div className="flex items-center gap-2 border-t border-border/60 px-3.5 py-2 text-[11px] text-muted-foreground">
        <Info className="h-3 w-3 shrink-0" strokeWidth={1.4} aria-hidden />
        Subagents report back to this thread when finished. Steering messages go
        to the orchestrator.
      </div>
    </div>
  );
});

function SubagentRow({
  sub,
  now,
  open,
  onToggle,
  onEnter,
}: {
  sub: SubagentView;
  now: number;
  open: boolean;
  onToggle: () => void;
  onEnter: () => void;
}) {
  const running = isRunning(sub);
  const tone = statusTone(sub.status);
  const activity = subagentActivityLine(sub);
  const meta = subagentMetaLine(sub, now);
  const peek = recentToolCalls(sub, 3);

  return (
    <div
      className={cn(
        "mb-0.5 overflow-hidden rounded-[10px]",
        open && "bg-foreground/[0.03]",
      )}
    >
      <div
        role="button"
        tabIndex={0}
        aria-expanded={open}
        onClick={onToggle}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            onToggle();
          }
        }}
        className="flex cursor-pointer items-center gap-2.5 rounded-[10px] px-2.5 py-2.5 hover:bg-foreground/[0.04]"
      >
        {/* Status glyph */}
        <span className="flex h-[22px] w-[22px] shrink-0 items-center justify-center">
          {running ? (
            <LoaderCircle
              className={cn("h-[17px] w-[17px] animate-spin", tone.text)}
              strokeWidth={1.9}
              aria-hidden
            />
          ) : sub.status === "failed" ? (
            <X className={cn("h-[18px] w-[18px]", tone.text)} strokeWidth={1.8} aria-hidden />
          ) : sub.status === "interrupted" ? (
            // Muted-amber non-spinning glyph — settled-but-unresolved, not
            // a green success check and not a red failure ✕.
            <Ban className={cn("h-[17px] w-[17px]", tone.text)} strokeWidth={1.8} aria-hidden />
          ) : (
            <Check
              className={cn("h-[18px] w-[18px]", tone.text)}
              strokeWidth={1.8}
              aria-hidden
            />
          )}
        </span>

        {/* Name + model column */}
        <div className="flex w-[150px] shrink-0 flex-col gap-0.5">
          <span className="truncate text-[13px] font-bold text-foreground">
            {sub.name ?? sub.agentType ?? "Subagent"}
          </span>
          {sub.model && (
            <span className="truncate font-mono text-[10px] text-muted-foreground">
              {sub.model}
            </span>
          )}
        </div>

        {/* Live activity / result line */}
        <div className="min-w-0 flex-1 overflow-hidden">
          {running ? (
            <span className="shimmer block truncate font-mono text-[12px]">
              {activity}
            </span>
          ) : (
            <span className="block truncate text-[12px] text-muted-foreground">
              {activity}
            </span>
          )}
        </div>

        {/* Meta */}
        <span className="shrink-0 font-mono text-[11px] text-muted-foreground">
          {meta}
        </span>

        {/* Enter */}
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            onEnter();
          }}
          className="flex h-[26px] shrink-0 items-center gap-1.5 rounded-[7px] border border-border bg-background px-2.5 text-[11px] font-semibold text-muted-foreground hover:border-muted-foreground/60 hover:text-foreground"
        >
          Enter
          <ChevronRight className="h-3 w-3" strokeWidth={1.7} aria-hidden />
        </button>

        {/* Chevron */}
        <ChevronDown
          className={cn(
            "h-3 w-3 shrink-0 text-muted-foreground transition-transform",
            open && "rotate-180",
          )}
          strokeWidth={1.6}
          aria-hidden
        />
      </div>

      {/* Inline peek */}
      {open && (
        <div className="flex flex-col gap-1 border-t border-border/60 py-2.5 pl-[43px] pr-3">
          <div className="mb-1 font-mono text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
            Recent activity
          </div>
          {peek.length === 0 ? (
            <div className="font-mono text-[11px] text-muted-foreground">
              No tool activity yet.
            </div>
          ) : (
            peek.map((tool) => {
              const d = describeToolCall(tool);
              return (
                <div
                  key={tool.id}
                  className="flex min-w-0 items-center gap-2.5 font-mono text-[11px] text-muted-foreground"
                >
                  <span className="shrink-0 text-muted-foreground/70">
                    {d.verb}
                  </span>
                  <span className="min-w-0 flex-1 truncate text-foreground/80">
                    {d.target}
                  </span>
                  {d.meta && (
                    <span className="shrink-0 text-muted-foreground/70">
                      {d.meta}
                    </span>
                  )}
                </div>
              );
            })
          )}
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation();
              onEnter();
            }}
            className="mt-2 flex h-7 items-center gap-1.5 self-start rounded-lg bg-foreground/[0.09] px-3 text-[12px] font-semibold text-foreground hover:bg-foreground/[0.14]"
          >
            Enter subagent
            <ChevronRight className="h-3 w-3" strokeWidth={1.7} aria-hidden />
          </button>
        </div>
      )}
    </div>
  );
}

/** 1s tick while `active`, so derived elapsed times advance without a
 *  provider `duration_ms`. Frozen (no interval) when nothing is running. */
function useNow(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [active]);
  return now;
}
