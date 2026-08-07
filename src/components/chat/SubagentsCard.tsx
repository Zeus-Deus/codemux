import { Ban, Check, ChevronDown, ChevronRight, X } from "lucide-react";
import { memo, useState } from "react";

import { AgentOrb } from "@/components/ui/agent-orb";
import { subagentOrbActivity } from "@/lib/agent-chat/orb-activity";
import {
  formatElapsed,
  isRunning,
  statusTone,
  subagentActivityLine,
  subagentGroupRollup,
  subagentLatestOutput,
  subagentMetaLine,
} from "@/lib/agent-chat/subagents";
import type { SubagentRunItem, SubagentView } from "@/lib/agent-chat/types";
import { cn } from "@/lib/utils";

import { TickingText } from "./TickingText";
import { formatCompactTokens } from "./WorkflowRunCard";

/**
 * Subagents in the thread (design 1b, "Subagents in the thread").
 *
 * There is no card here any more: no border box, no tinted rows, no
 * `Enter ›` buttons. A spawn group is a 1.5px vertical gradient **rail**
 * carrying the group's state (accent while running, success once settled)
 * with the content sitting to its right, so the block reads as part of the
 * transcript rather than a widget dropped into it. Rows are plain hover
 * targets that expand inline; status is a rail and a glyph, never a filled
 * row.
 *
 * Three shapes, one component:
 * - **running** — rail + header + rows, each running row owning its own
 *   activity-matched orb (the whole-run orb lives on the composer strip).
 * - **settled, collapsed** (the resting state once the run is over) — a
 *   single ~30px hover row: check · "Ran N subagents" · mono rollup · View.
 * - **settled, expanded** — clicking that line re-opens the rail view with
 *   settled styling, so the detail is one click away and never in the way.
 *
 * Rendered as a single memoized transcript row: a streaming child event
 * mutates only this group's `item` reference, so exactly this row
 * re-renders (issue #77 scroller contract preserved). Elapsed labels tick
 * through `TickingText`, which writes `textContent` directly and never
 * re-enters React.
 *
 * The root carries `data-subagent-card={item.id}` so MessageList can apply
 * the post-navigation highlight after LegendList mounts the requested row.
 *
 * Prototype sizes are half-pixel (12.5 / 11.5 / 10.5px); they are rounded
 * to the whole-pixel compact scale here per the design system — half-pixel
 * font sizes soften glyphs badly in the WebKitGTK desktop renderer.
 */
export const SubagentsCard = memo(function SubagentsCard({
  item,
  onEnter,
}: {
  item: SubagentRunItem;
  onEnter: (subagentId: string) => void;
}) {
  const subs = item.subagents;
  // `pending` counts as live: a queued subagent has not settled, so the
  // group must not collapse to its "Ran N subagents" resting line yet.
  const anyRunning = subs.some((s) => isRunning(s));
  const anyFailed = subs.some((s) => s.status === "failed");
  const anyHalted = subs.some(
    (s) => s.status === "interrupted" || s.status === "stopped",
  );
  const [openId, setOpenId] = useState<string | null>(null);
  const [groupOpen, setGroupOpen] = useState(false);

  if (!anyRunning && !groupOpen) {
    return (
      <div data-subagent-card={item.id}>
        <SettledLine
          subs={subs}
          anyFailed={anyFailed}
          anyHalted={anyHalted}
          onOpen={() => setGroupOpen(true)}
        />
      </div>
    );
  }

  return (
    <div data-subagent-card={item.id} className="flex">
      {/* Group rail: the card border's replacement. Carries the group's
          state as a top-to-transparent gradient — accent while work is in
          flight, success (or attention / halted) once it settles. */}
      <div
        data-testid="subagent-group-rail"
        data-state={anyRunning ? "running" : anyFailed ? "failed" : "settled"}
        className={cn(
          "w-[1.5px] shrink-0 rounded-full bg-gradient-to-b",
          anyRunning
            ? "from-accent-ember to-accent-ember/[0.12]"
            : anyFailed
              ? "from-status-attention to-status-attention/10"
              : anyHalted
                ? "from-status-working/80 to-status-working/10"
                : "from-status-open to-status-open/10",
        )}
        aria-hidden
      />

      <div className="min-w-0 flex-1 pl-[15px]">
        {/* Header line */}
        <div className="flex h-6 items-center gap-2.5">
          <span className="text-[13px] font-bold text-foreground">
            Subagents
          </span>
          <span className="text-[12px] text-muted-foreground">
            {subs.length} {subs.length === 1 ? "task" : "tasks"}
            {anyRunning ? " · running in parallel" : " · complete"}
          </span>
          <span className="ml-auto shrink-0 font-mono text-[11px] text-muted-foreground">
            {subs.filter((s) => !isRunning(s)).length} done ·{" "}
            {subs.filter((s) => isRunning(s)).length} active
          </span>
        </div>

        {/* Rows. The negative margin lets a hover target breathe past the
            content box without indenting the text away from the rail. */}
        <div className="-mx-2 mt-1.5 flex flex-col">
          {subs.map((sub) => (
            <SubagentRow
              key={sub.id}
              sub={sub}
              open={openId === sub.id}
              // One row expanded at a time (design behavior).
              onToggle={() =>
                setOpenId((cur) => (cur === sub.id ? null : sub.id))
              }
              onEnter={() => onEnter(sub.id)}
            />
          ))}
        </div>

        <div className="px-px pt-2 text-[11px] leading-normal text-muted-foreground">
          Subagents report back to this thread when they finish. Steering
          messages go to the orchestrator.
        </div>
      </div>
    </div>
  );
});

/**
 * The settled group's resting shape: one ~30px hover row. Clicking it
 * expands the rail view — the detail never disappears, it just stops
 * occupying the transcript once nobody is waiting on it.
 */
function SettledLine({
  subs,
  anyFailed,
  anyHalted,
  onOpen,
}: {
  subs: SubagentView[];
  anyFailed: boolean;
  anyHalted: boolean;
  onOpen: () => void;
}) {
  return (
    <div
      data-testid="subagent-group-settled"
      role="button"
      tabIndex={0}
      aria-expanded={false}
      onClick={onOpen}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onOpen();
        }
      }}
      className="-mx-2.5 flex h-[30px] cursor-pointer items-center gap-2.5 rounded-md px-2.5 hover:bg-foreground/[0.05]"
    >
      {anyFailed ? (
        <X
          className="h-3.5 w-3.5 shrink-0 text-status-attention"
          strokeWidth={1.8}
          aria-hidden
        />
      ) : anyHalted ? (
        <Ban
          className="h-3.5 w-3.5 shrink-0 text-status-working/80"
          strokeWidth={1.8}
          aria-hidden
        />
      ) : (
        <Check
          className="h-3.5 w-3.5 shrink-0 text-status-open"
          strokeWidth={1.8}
          aria-hidden
        />
      )}
      <span className="truncate text-[13px] font-semibold text-foreground/80">
        Ran {subs.length} subagent{subs.length === 1 ? "" : "s"}
      </span>
      <TickingText
        className="ml-auto shrink-0 whitespace-nowrap font-mono text-[11px] text-muted-foreground"
        // The run is over: freeze the label rather than tick a static
        // duration once a second for the rest of the session.
        active={false}
        compute={(now) => groupRollupLabel(subs, now)}
      />
      <span className="shrink-0 font-mono text-[11px] text-foreground/80">
        View
      </span>
      <ChevronDown
        className="h-2.5 w-2.5 shrink-0 text-muted-foreground"
        strokeWidth={1.8}
        aria-hidden
      />
    </div>
  );
}

/**
 * Whole-group mono rollup ("1m 14s · Σ 38.8k · 9 tools").
 *
 * Every segment is dropped rather than faked when the providers never
 * reported it — a group whose subagents carried no usage prints the
 * duration and the tool count and stops there.
 */
export function groupRollupLabel(
  subs: readonly SubagentView[],
  now: number,
): string {
  const rollup = subagentGroupRollup(subs, now);
  const parts: string[] = [];
  if (rollup.elapsedMs != null) parts.push(formatElapsed(rollup.elapsedMs));
  if (rollup.totalTokens != null) {
    parts.push(`Σ ${formatCompactTokens(rollup.totalTokens)}`);
  }
  parts.push(
    `${rollup.toolCount} ${rollup.toolCount === 1 ? "tool" : "tools"}`,
  );
  return parts.join(" · ");
}

function SubagentRow({
  sub,
  open,
  onToggle,
  onEnter,
}: {
  sub: SubagentView;
  open: boolean;
  onToggle: () => void;
  onEnter: () => void;
}) {
  const running = isRunning(sub);
  const tone = statusTone(sub.status);
  const summary = subagentActivityLine(sub);
  const output = subagentLatestOutput(sub);

  return (
    <div>
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
        className="flex cursor-pointer items-center gap-[11px] rounded-md px-[9px] py-[7px] hover:bg-foreground/[0.05]"
      >
        {/* Status glyph — 20px slot. Running rows animate (one orb per
            live thing); everything settled drops back to a flat glyph. */}
        <span className="flex h-5 w-5 shrink-0 items-center justify-center">
          {running ? (
            <AgentOrb size={20} {...subagentOrbActivity(sub)} aria-hidden />
          ) : sub.status === "failed" ? (
            <X
              className={cn("h-3.5 w-3.5", tone.text)}
              strokeWidth={1.8}
              aria-hidden
            />
          ) : sub.status === "interrupted" || sub.status === "stopped" ? (
            // Settled-but-unresolved: neither a success check nor a
            // failure ✕.
            <Ban
              className={cn("h-3.5 w-3.5", tone.text)}
              strokeWidth={1.8}
              aria-hidden
            />
          ) : (
            <Check
              className={cn("h-3.5 w-3.5", tone.text)}
              strokeWidth={1.8}
              aria-hidden
            />
          )}
        </span>

        <span className="w-[132px] shrink-0 truncate text-[13px] font-semibold text-foreground">
          {sub.name ?? sub.agentType ?? "Subagent"}
        </span>

        <span className="min-w-0 flex-1 truncate text-[12px] text-muted-foreground">
          {summary}
        </span>

        <TickingText
          className="shrink-0 whitespace-nowrap font-mono text-[11px] text-muted-foreground"
          active={running}
          compute={(now) => subagentMetaLine(sub, now)}
        />

        <ChevronRight
          className={cn(
            "h-3 w-3 shrink-0 text-muted-foreground/70 transition-transform",
            open && "rotate-90",
          )}
          strokeWidth={1.6}
          aria-hidden
        />
      </div>

      {open && (
        <div className="rise-in mt-px mr-[9px] mb-[7px] ml-[34px] flex flex-col gap-2 border-l-[1.5px] border-border py-2 pr-1 pl-3">
          <pre className="m-0 font-mono text-[11px] leading-[1.75] whitespace-pre-wrap text-muted-foreground">
            {output ?? "No output yet."}
          </pre>
          <div className="flex items-center gap-3.5">
            {/* Replaces the old `Enter ›` button: same drill-in, as a
                text button that belongs to the expansion instead of a
                control competing with the row. */}
            <button
              type="button"
              onClick={(e) => {
                e.stopPropagation();
                onEnter();
              }}
              className="flex items-center gap-1 text-[12px] font-semibold text-accent-ember hover:text-accent-ember/80"
            >
              Open thread
              <ChevronRight className="h-3 w-3" strokeWidth={1.7} aria-hidden />
            </button>
            {sub.model && (
              <span className="truncate font-mono text-[10px] text-muted-foreground">
                {sub.model}
              </span>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
