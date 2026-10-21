import { ArrowRight, Bot, HelpCircle, Lightbulb, ListTree } from "lucide-react";
import { ContribBoundary, ContribRender } from "@/contrib/react/boundary";
import { usePluginToolRenderer } from "@/contrib/tool-renderers";
import { memo, useState } from "react";

import {
  Trace,
  TraceCollapsible,
  TraceCollapsibleContent,
  TraceCollapsibleTrigger,
  TraceDescription,
  TraceIcon,
  TraceLine,
  TraceTitle,
  type TraceStatus,
} from "@/components/ui/trace";
import { presentationToolName } from "@/lib/agent-chat/native-tool-call";
import type { SubagentRunItem } from "@/lib/agent-chat/types";
import { cn } from "@/lib/utils";
import { useUIStore } from "@/stores/ui-store";

import type { ActivityStep, WorkEntry } from "./transcript-slots";
import { TickingText } from "./TickingText";
import { ToolCallBody } from "./ToolCallBodies";
import {
  isSubagentRun,
  stepStatus,
  subagentPreview,
  subagentRunMeta,
  subagentRunStatus,
  traceStepDescription,
  traceStepTitle,
  workLogTotals,
  type StepStatus,
  type WorkLogTotals,
} from "./activity-steps";
import { StepElapsed } from "./StepElapsed";
import { toolIcon } from "./tool-visuals";

/** The opened history shows this many recent entries before "Show earlier". */
export const WORK_LOG_HISTORY_WINDOW = 10;

/** Durations, counts and outcome notes: data, but not code, so sans. */
const META_CLASS = "text-label tabular-nums";

/**
 * One line for one uninterrupted stretch of mechanical work: tool calls,
 * thoughts and subagent runs alike, drawn with shadcn's Trace. Collapsed and
 * live, the line names the running tool or reads "Thinking…"; settled, it is
 * the newest entry plus stretch totals. Pressing it opens the chronological
 * history as nested Trace lines. There is deliberately no
 * surrounding card, status banner or settled header—the turn fold owns
 * completion and the final assistant answer owns the hierarchy.
 *
 * Every disclosure here is the user's own choice, so the block never closes
 * itself when the turn ends: someone reading a diff mid-run keeps reading it.
 * `onKeepOpen` tells the transcript that the user opened this log while the
 * turn ran, so the turn fold that settles over it stays expanded too. That
 * holds for every log in the running turn (`turnLive`), not just the one with
 * the live row: agents often write a message between two stretches of work.
 */
export const ActivityBlock = memo(function ActivityBlock({
  items,
  working,
  turnLive = false,
  workspaceId,
  onKeepOpen,
}: {
  items: WorkEntry[];
  working: boolean;
  /** The block belongs to the turn still running (implied by `working`). */
  turnLive?: boolean;
  workspaceId?: string | null;
  /** Keyed by the block's first entry id, which is stable for its life. */
  onKeepOpen?: (entryId: string, keep: boolean) => void;
}) {
  const [open, setOpen] = useState(false);
  const [showAll, setShowAll] = useState(false);
  const [expandedStepId, setExpandedStepId] = useState<string | null>(null);

  const latest = items[items.length - 1];
  if (!latest) return null;
  const blockId = items[0].id;
  // Only a live turn needs the hint. A settled block already sits under an
  // expanded fold, and dropping the hint there would fold the turn away
  // under the cursor.
  const keepOpen = (keep: boolean) => {
    if (working || turnLive) onKeepOpen?.(blockId, keep);
  };

  const runs = items.filter(isSubagentRun);

  // `inHistory`: the entry is a row of the opened log rather than the
  // block's one collapsed line. The live line stays quiet; the history the
  // user opened shows each running step's own clock.
  const renderEntry = (entry: WorkEntry, inHistory = false) => {
    const live = working && !inHistory && entry.id === latest.id;
    if (isSubagentRun(entry)) {
      return (
        <SubagentRunLine
          key={entry.id}
          run={entry}
          workspaceId={workspaceId}
        />
      );
    }
    return (
      <StepLine
        key={entry.id}
        step={entry}
        live={live}
        turnLive={working || turnLive}
        inHistory={inHistory}
        expanded={expandedStepId === entry.id}
        onToggle={(expanding) => {
          setExpandedStepId(expanding ? entry.id : null);
          // A lone step is the whole log, so its detail is what to keep.
          if (items.length === 1) keepOpen(expanding);
        }}
      />
    );
  };

  let body: React.ReactNode;
  if (items.length === 1) {
    body = renderEntry(latest);
  } else {
    const hiddenCount =
      open && !showAll ? Math.max(0, items.length - WORK_LOG_HISTORY_WINDOW) : 0;
    const totals = workLogTotals(items);
    body = (
      <TraceCollapsible
        open={open}
        onOpenChange={(next) => {
          setOpen(next);
          if (!next) {
            setShowAll(false);
            setExpandedStepId(null);
          }
          keepOpen(next);
        }}
      >
        <TraceCollapsibleTrigger>
          {open ? (
            <TraceLine>
              <TraceIcon>
                <ListTree />
              </TraceIcon>
              <TraceTitle className="font-medium text-foreground/80">Work log</TraceTitle>
              <TotalsLabel totals={totals} />
            </TraceLine>
          ) : (
            <CollapsedLine entry={latest} live={working} totals={totals} />
          )}
        </TraceCollapsibleTrigger>
        <TraceCollapsibleContent>
          <Trace>
            {hiddenCount > 0 && (
              <button type="button" onClick={() => setShowAll(true)}>
                <TraceLine className="text-muted-foreground/70">
                  <TraceTitle>Show {hiddenCount} earlier</TraceTitle>
                </TraceLine>
              </button>
            )}
            {items.slice(hiddenCount).map((entry) => renderEntry(entry, true))}
          </Trace>
        </TraceCollapsibleContent>
      </TraceCollapsible>
    );
  }

  return (
    <div
      className="-mx-1 select-text px-1 py-0.5"
      data-subagent-card={runs[0]?.id}
    >
      {/* Subagent jump requests address run ids. These zero-size anchors keep
          every run in the stretch findable while the line is collapsed. */}
      {runs.map((run) => (
        <span key={run.id} data-subagent-run-id={run.id} className="hidden" />
      ))}
      <Trace>{body}</Trace>
    </div>
  );
});

function traceStatus(status: StepStatus): TraceStatus | undefined {
  if (status === "running") return "running";
  if (status === "error") return "error";
  return undefined;
}

/** Icon slot for a step: the tool's own glyph, a bulb for a thought. */
function StepIcon({ step }: { step: ActivityStep }) {
  if (step.kind !== "reasoning" && step.status === "unconfirmed") {
    return (
      <TraceIcon title="No completion result was reported">
        <HelpCircle />
      </TraceIcon>
    );
  }
  const Icon = step.kind === "reasoning" ? Lightbulb : toolIcon(presentationToolName(step));
  return (
    <TraceIcon>
      <Icon />
    </TraceIcon>
  );
}

/**
 * The live line names a tool only while that tool runs. A thought, or the
 * gap after a step settles, reads as one steady "Thinking…" instead of
 * streaming the thought's text or flashing the last call's past tense.
 */
function isLiveThinking(step: ActivityStep, live: boolean): boolean {
  return live && (step.kind === "reasoning" || stepStatus(step) !== "running");
}

function ThinkingLine() {
  return (
    <TraceLine status="running">
      <TraceIcon>
        <Lightbulb />
      </TraceIcon>
      <TraceTitle>Thinking…</TraceTitle>
    </TraceLine>
  );
}

/** Verb in prose, target in mono, argument dimmed: "Ran `cargo test`". */
function StepTitle({ step }: { step: ActivityStep }) {
  const { verb, target, argument } = traceStepTitle(step);
  return (
    <TraceTitle>
      {verb}
      {target ? (
        <>
          {verb ? " " : null}
          <span className={cn(step.kind === "reasoning" ? "italic" : "font-mono text-label")}>
            {target}
          </span>
        </>
      ) : null}
      {argument ? (
        <>
          {" "}
          <span className="font-mono text-label opacity-60">{argument}</span>
        </>
      ) : null}
    </TraceTitle>
  );
}

function CollapsedLine({
  entry,
  live,
  totals,
}: {
  entry: WorkEntry;
  live: boolean;
  totals: WorkLogTotals;
}) {
  // The stretch totals count up with every call, so they wait for the
  // stretch to settle.
  const settledTotals = live ? null : <TotalsLabel totals={totals} />;
  if (isSubagentRun(entry)) {
    const status = subagentRunStatus(entry);
    return (
      <TraceLine status={traceStatus(status)}>
        <TraceIcon>
          <Bot />
        </TraceIcon>
        <TraceTitle>
          {status === "running" ? "Running subagents" : "Ran subagents"}{" "}
          <span className="opacity-60">{subagentPreview(entry.subagents)}</span>
        </TraceTitle>
        {settledTotals}
      </TraceLine>
    );
  }
  if (isLiveThinking(entry, live)) return <ThinkingLine />;
  return (
    <TraceLine status={traceStatus(stepStatus(entry))}>
      <StepIcon step={entry} />
      <StepTitle step={entry} />
      {stepStatus(entry) === "unconfirmed" ? (
        <TraceDescription className={META_CLASS}>outcome unconfirmed</TraceDescription>
      ) : null}
      {settledTotals}
    </TraceLine>
  );
}

function TotalsLabel({ totals }: { totals: WorkLogTotals }) {
  if (!totals.label && totals.failed === 0) return null;
  return (
    <TraceDescription className={META_CLASS}>
      {totals.label}
      {totals.failed > 0 ? (
        <span className="text-status-attention">
          {totals.label ? " · " : ""}
          {totals.failed} failed
        </span>
      ) : null}
    </TraceDescription>
  );
}

function StepLine({
  step,
  live,
  turnLive,
  inHistory,
  expanded,
  onToggle,
}: {
  step: ActivityStep;
  live: boolean;
  /** The step's turn is still running, so a `running` step really is. A
   *  settled transcript can hold a call that never reported back (crash,
   *  quit); its start time is no longer meaningful, so it keeps the static
   *  meta instead of counting forever. */
  turnLive: boolean;
  /** A row of the opened log, where a running step shows its elapsed time. */
  inHistory: boolean;
  expanded: boolean;
  onToggle: (expanding: boolean) => void;
}) {
  const status = stepStatus(step);
  const description = traceStepDescription(step);
  const runningLive = turnLive && status === "running";
  const ticking = inHistory && runningLive && step.started_at != null;
  return (
    <TraceCollapsible open={expanded} onOpenChange={onToggle}>
      <TraceCollapsibleTrigger>
        {isLiveThinking(step, live) ? (
          <ThinkingLine />
        ) : (
          <TraceLine status={traceStatus(status)}>
            <StepIcon step={step} />
            <StepTitle step={step} />
            {ticking ? (
              <StepElapsed step={step} className={cn(META_CLASS, "text-muted-foreground/60")} />
            ) : description && !runningLive ? (
              <TraceDescription className={META_CLASS}>{description}</TraceDescription>
            ) : null}
          </TraceLine>
        )}
      </TraceCollapsibleTrigger>
      <TraceCollapsibleContent>
        {step.kind === "reasoning" ? (
          <p className="whitespace-pre-wrap break-words text-body italic leading-[1.6] text-muted-foreground">
            {step.text}
          </p>
        ) : (
          <StepBody step={step} />
        )}
      </TraceCollapsibleContent>
    </TraceCollapsible>
  );
}

/** A tool step's opened body: a plugin renderer for this tool when one is
 *  enabled (`transcript.toolRenderer`), otherwise the core body. */
function StepBody({ step }: { step: Extract<ActivityStep, { kind: "tool_call" }> }) {
  const plugin = usePluginToolRenderer(step.tool_name);
  if (plugin) {
    return (
      <ContribBoundary id={plugin.id} variant="block">
        <ContribRender render={plugin.render} props={{ call: step }} />
      </ContribBoundary>
    );
  }
  return <ToolCallBody item={step} />;
}

/** A subagent run inside the work log. Its detail lives in the Subagents
 *  panel, so the line opens that panel instead of expanding inline. */
function SubagentRunLine({
  run,
  workspaceId,
}: {
  run: SubagentRunItem;
  workspaceId?: string | null;
}) {
  const selected = useUIStore((state) =>
    workspaceId ? state.rightPanelTabs[workspaceId] === "subagents" : false,
  );
  const setRightPanelTab = useUIStore((state) => state.setRightPanelTab);
  const status = subagentRunStatus(run);
  const count = run.subagents.length;
  return (
    <button
      type="button"
      onClick={() => {
        if (workspaceId) setRightPanelTab(workspaceId, "subagents");
      }}
      disabled={!workspaceId}
      aria-pressed={selected}
      aria-label={`View ${count} subagent${count === 1 ? "" : "s"}`}
      className={cn("group/subagents disabled:cursor-default", selected && "text-foreground")}
    >
      <TraceLine status={traceStatus(status)} className={cn(selected && "text-foreground")}>
        <TraceIcon>
          <Bot />
        </TraceIcon>
        <TraceTitle>
          {status === "running" ? "Running" : "Ran"} {count} subagent{count === 1 ? "" : "s"}{" "}
          <span className="opacity-60">{subagentPreview(run.subagents)}</span>
        </TraceTitle>
        <TickingText
          active={status === "running"}
          className={cn(META_CLASS, "shrink-0 whitespace-nowrap text-muted-foreground/60 tabular-nums")}
          compute={(now) => subagentRunMeta(run, now)}
        />
        <TraceIcon>
          <ArrowRight className="size-3 transition-transform duration-150 group-hover/subagents:translate-x-0.5" />
        </TraceIcon>
      </TraceLine>
    </button>
  );
}
