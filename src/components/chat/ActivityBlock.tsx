import { Check, ChevronDown, ChevronRight, LoaderCircle, X } from "lucide-react";
import { memo, useEffect, useRef, useState } from "react";

import { AgentOrb } from "@/components/ui/agent-orb";
import { turnOrbActivity } from "@/lib/agent-chat/orb-activity";
import type { SubagentRunItem } from "@/lib/agent-chat/types";
import { cn } from "@/lib/utils";
import { useUIStore } from "@/stores/ui-store";

import type { ActivityStep, WorkEntry } from "./transcript-slots";
import { TickingText } from "./TickingText";
import { ToolCallBody } from "./ToolCallBodies";
import {
  isSubagentRun,
  subagentPreview,
  subagentRunMeta,
  subagentRunStatus,
  toStepView,
  workLogTotals,
  type StepStatus,
  type WorkLogTotals,
} from "./activity-steps";

/** The opened history shows this many recent entries before "Show earlier". */
export const WORK_LOG_HISTORY_WINDOW = 10;

const ROW_CLASS =
  "flex w-full min-w-0 items-center gap-1.5 rounded-md px-0.5 py-0.5 text-left text-body-sm leading-5 transition-colors hover:bg-surface-2 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/70";

type OrbActivity = ReturnType<typeof turnOrbActivity>;

/**
 * One line for one uninterrupted stretch of mechanical work: tool calls,
 * thoughts and subagent runs alike. Collapsed, the line is the newest entry
 * plus stretch totals; pressing it opens the chronological history. There is
 * deliberately no surrounding card, status banner or settled header—the turn
 * fold owns completion and the final assistant answer owns the hierarchy.
 */
export const ActivityBlock = memo(function ActivityBlock({
  items,
  working,
  workspaceId,
}: {
  items: WorkEntry[];
  working: boolean;
  workspaceId?: string | null;
}) {
  const [open, setOpen] = useState(false);
  const [showAll, setShowAll] = useState(false);
  const [expandedStepId, setExpandedStepId] = useState<string | null>(null);
  const previousWorking = useRef(working);

  useEffect(() => {
    if (previousWorking.current && !working) {
      setOpen(false);
      setShowAll(false);
      setExpandedStepId(null);
    }
    previousWorking.current = working;
  }, [working]);

  const latest = items[items.length - 1];
  if (!latest) return null;

  const runs = items.filter(isSubagentRun);
  const orbActivity = working ? turnOrbActivity(items) : undefined;

  const renderEntry = (entry: WorkEntry) => {
    const live = working && entry.id === latest.id;
    if (isSubagentRun(entry)) {
      return (
        <SubagentRunRow
          key={entry.id}
          run={entry}
          live={live}
          workspaceId={workspaceId}
        />
      );
    }
    return (
      <StepRow
        key={entry.id}
        step={entry}
        live={live}
        orbActivity={live ? orbActivity : undefined}
        expanded={expandedStepId === entry.id}
        onToggle={() =>
          setExpandedStepId((current) => (current === entry.id ? null : entry.id))
        }
      />
    );
  };

  let body: React.ReactNode;
  if (items.length === 1) {
    body = renderEntry(latest);
  } else if (!open) {
    body = (
      <CollapsedLine
        entry={latest}
        live={working}
        orbActivity={orbActivity}
        totals={workLogTotals(items)}
        onOpen={() => setOpen(true)}
      />
    );
  } else {
    const hiddenCount = showAll
      ? 0
      : Math.max(0, items.length - WORK_LOG_HISTORY_WINDOW);
    body = (
      <>
        <button
          type="button"
          aria-expanded
          onClick={() => {
            setOpen(false);
            setShowAll(false);
            setExpandedStepId(null);
          }}
          className={ROW_CLASS}
        >
          <span className="flex size-5 shrink-0 items-center justify-center text-muted-foreground/65">
            <ChevronDown className="size-3.5 rotate-180" aria-hidden />
          </span>
          <span className="min-w-0 flex-1 font-medium text-foreground/80">
            Work log
          </span>
          <TotalsLabel totals={workLogTotals(items)} />
        </button>
        {hiddenCount > 0 && (
          <button
            type="button"
            onClick={() => setShowAll(true)}
            className={cn(ROW_CLASS, "text-muted-foreground")}
          >
            <span className="size-5 shrink-0" aria-hidden />
            Show {hiddenCount} earlier
          </button>
        )}
        <div className="space-y-px">
          {items.slice(hiddenCount).map(renderEntry)}
        </div>
      </>
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
      {body}
    </div>
  );
});

function CollapsedLine({
  entry,
  live,
  orbActivity,
  totals,
  onOpen,
}: {
  entry: WorkEntry;
  live: boolean;
  orbActivity?: OrbActivity;
  totals: WorkLogTotals;
  onOpen: () => void;
}) {
  const { verb, summary, status } = isSubagentRun(entry)
    ? {
        verb: "agents",
        summary: subagentPreview(entry.subagents),
        status: subagentRunStatus(entry),
      }
    : toStepView(entry);
  return (
    <button
      type="button"
      aria-expanded={false}
      onClick={onOpen}
      className={ROW_CLASS}
    >
      <span className="flex size-5 shrink-0 items-center justify-center">
        {live ? (
          <AgentOrb size={20} {...(orbActivity ?? {})} aria-hidden />
        ) : (
          <StepGlyph status={status} />
        )}
      </span>
      <span className="shrink-0 text-muted-foreground/65">{verb}</span>
      <span className="min-w-0 flex-1 truncate font-mono text-label text-muted-foreground">
        {summary}
      </span>
      <TotalsLabel totals={totals} />
      <ChevronDown
        className="size-3 shrink-0 text-muted-foreground/45"
        aria-hidden
      />
    </button>
  );
}

function TotalsLabel({ totals }: { totals: WorkLogTotals }) {
  if (!totals.label && totals.failed === 0) return null;
  return (
    <span className="shrink-0 font-mono text-caption text-muted-foreground/55">
      {totals.label}
      {totals.failed > 0 ? (
        <span className="text-status-attention">
          {totals.label ? " · " : ""}
          {totals.failed} failed
        </span>
      ) : null}
    </span>
  );
}

function StepRow({
  step,
  live,
  orbActivity,
  expanded,
  onToggle,
}: {
  step: ActivityStep;
  live: boolean;
  orbActivity?: OrbActivity;
  expanded: boolean;
  onToggle: () => void;
}) {
  const view = toStepView(step);
  return (
    <div>
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={expanded}
        className={ROW_CLASS}
      >
        <span className="flex size-5 shrink-0 items-center justify-center">
          {live ? (
            <AgentOrb size={20} {...(orbActivity ?? {})} aria-hidden />
          ) : (
            <StepGlyph status={view.status} />
          )}
        </span>
        <span className="shrink-0 text-muted-foreground/65">{view.verb}</span>
        <span className="min-w-0 flex-1 truncate font-mono text-label text-muted-foreground">
          {view.summary}
        </span>
        {view.meta && !live ? (
          <span
            className={cn(
              "shrink-0 font-mono text-caption text-muted-foreground/55",
              view.status === "error" && "text-status-attention",
            )}
          >
            {view.meta}
          </span>
        ) : null}
        <ChevronDown
          className={cn(
            "size-3 shrink-0 text-muted-foreground/45 transition-transform",
            expanded && "rotate-180",
          )}
          aria-hidden
        />
      </button>
      {expanded && (
        <div className="ml-[10px] mt-0.5 border-l border-border/60 py-1.5 pl-3">
          {step.kind === "reasoning" ? (
            <p className="whitespace-pre-wrap break-words text-body italic leading-[1.6] text-muted-foreground">
              {step.text}
            </p>
          ) : (
            <ToolCallBody item={step} />
          )}
        </div>
      )}
    </div>
  );
}

/** A subagent run inside the work log. Its detail lives in the Subagents
 *  panel, so the row opens that panel instead of expanding inline. */
function SubagentRunRow({
  run,
  live,
  workspaceId,
}: {
  run: SubagentRunItem;
  live: boolean;
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
      className={cn(
        ROW_CLASS,
        "group/subagents disabled:cursor-default",
        selected && "bg-surface-3",
      )}
    >
      <span className="flex size-5 shrink-0 items-center justify-center">
        {live ? <AgentOrb size={20} aria-hidden /> : <StepGlyph status={status} />}
      </span>
      <span className="shrink-0 text-muted-foreground/65">agents</span>
      <span className="min-w-0 flex-1 truncate font-mono text-label text-muted-foreground">
        {subagentPreview(run.subagents)}
      </span>
      <TickingText
        active={status === "running"}
        className="shrink-0 whitespace-nowrap font-mono text-caption text-muted-foreground/55"
        compute={(now) => subagentRunMeta(run, now)}
      />
      <span className="flex shrink-0 items-center gap-0.5 text-caption font-medium text-foreground/70">
        View
        <ChevronRight
          className="size-3 transition-transform group-hover/subagents:translate-x-0.5"
          strokeWidth={1.7}
          aria-hidden
        />
      </span>
    </button>
  );
}

function StepGlyph({ status }: { status: StepStatus }) {
  if (status === "running") {
    return (
      <LoaderCircle
        className="size-3 animate-spin text-muted-foreground/70"
        strokeWidth={1.8}
        aria-hidden
      />
    );
  }
  if (status === "error") {
    return <X className="size-3 text-status-attention" strokeWidth={2} aria-hidden />;
  }
  return <Check className="size-3 text-status-open" strokeWidth={2} aria-hidden />;
}
