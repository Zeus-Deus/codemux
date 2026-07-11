import { Check, ChevronDown, CircleCheck, LoaderCircle, X } from "lucide-react";
import { memo, useEffect, useRef, useState } from "react";

import { cn } from "@/lib/utils";

import type { ActivityStep } from "./transcript-slots";
import { ToolCallBody } from "./ToolCallBodies";
import {
  deriveActivityDurationMs,
  deriveActivitySummary,
  deriveActivityCounts,
  deriveLiveAction,
  deriveWorkingCounter,
  formatActivityDuration,
  toStepView,
  type StepStatus,
} from "./activity-steps";

/**
 * Activity block (Activity Stream design). ONE card per contiguous run of
 * mechanical steps (reasoning + tool calls). Two derived states share the
 * same step list:
 *
 *  - **Working** (live tail of an active turn): a collapsed row — amber
 *    spinner, bold "Working", a shimmering mono live action, a mono
 *    `N done · M running` counter, a rotating chevron. Expanding shows the
 *    steps in place; completed steps dim (~0.6) with green checks, the
 *    running step full-opacity with an amber spinner.
 *  - **Settled**: rolls up to a green one-liner — circled check, a derived
 *    summary sentence, mono `N steps · 1m 12s` meta (duration when
 *    derivable), and a Details/Hide toggle. Collapsed by default; expanding
 *    shows the identical list (all green checks, full opacity).
 *
 * Clicking a step row expands the full tool detail inline beneath it
 * (`ToolCallBody` reuses `DiffView` for edits; thoughts show their text).
 * Modeled on ToolGroupCard's card idiom.
 */
export const ActivityBlock = memo(function ActivityBlock({
  items,
  working,
}: {
  items: ActivityStep[];
  working: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [expandedStepId, setExpandedStepId] = useState<string | null>(null);

  // On settle (working → false) reset any mid-stream expansion so the run
  // re-renders as the collapsed settled header (design contract).
  const prevWorking = useRef(working);
  useEffect(() => {
    if (prevWorking.current && !working) {
      setOpen(false);
      setExpandedStepId(null);
    }
    prevWorking.current = working;
  }, [working]);

  return (
    <div className="overflow-hidden rounded-[11px] border border-border/60 bg-muted/40">
      {working ? (
        <WorkingHeader steps={items} open={open} onToggle={() => setOpen((v) => !v)} />
      ) : (
        <SettledHeader steps={items} open={open} onToggle={() => setOpen((v) => !v)} />
      )}
      {open && (
        <div className="flex flex-col gap-px border-t border-border/60 px-3 py-2">
          {items.map((step) => (
            <StepRow
              key={step.id}
              step={step}
              working={working}
              expanded={expandedStepId === step.id}
              onToggle={() =>
                setExpandedStepId((cur) => (cur === step.id ? null : step.id))
              }
            />
          ))}
        </div>
      )}
    </div>
  );
});

function WorkingHeader({
  steps,
  open,
  onToggle,
}: {
  steps: ActivityStep[];
  open: boolean;
  onToggle: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onToggle}
      aria-expanded={open}
      className="flex w-full items-center gap-2.5 px-3 py-2.5 text-left"
    >
      <LoaderCircle
        className="h-[17px] w-[17px] shrink-0 animate-spin text-status-working"
        strokeWidth={1.9}
        aria-hidden
      />
      <span className="shrink-0 text-[12.5px] font-bold text-foreground">
        Working
      </span>
      <span className="min-w-0 flex-1 truncate">
        <span className="shimmer font-mono text-[11px]">
          {deriveLiveAction(steps)}
        </span>
      </span>
      <span className="shrink-0 font-mono text-[10.5px] text-muted-foreground">
        {deriveWorkingCounter(steps)}
      </span>
      <ChevronDown
        className={cn(
          "h-3 w-3 shrink-0 text-muted-foreground/70 transition-transform",
          open && "rotate-180",
        )}
        aria-hidden
      />
    </button>
  );
}

function SettledHeader({
  steps,
  open,
  onToggle,
}: {
  steps: ActivityStep[];
  open: boolean;
  onToggle: () => void;
}) {
  const { total, failed } = deriveActivityCounts(steps);
  const durationMs = deriveActivityDurationMs(steps);
  return (
    <button
      type="button"
      onClick={onToggle}
      aria-expanded={open}
      className="flex w-full items-center gap-2.5 px-3 py-2.5 text-left"
    >
      <CircleCheck
        className="h-4 w-4 shrink-0 text-status-open"
        strokeWidth={1.8}
        aria-hidden
      />
      <span className="min-w-0 flex-1 truncate text-[12.5px] font-semibold text-muted-foreground">
        {deriveActivitySummary(steps)}
      </span>
      <span className="shrink-0 font-mono text-[10.5px] text-muted-foreground">
        {total} step{total === 1 ? "" : "s"}
        {durationMs != null ? ` · ${formatActivityDuration(durationMs)}` : ""}
        {failed > 0 ? (
          <span className="text-status-attention"> · {failed} failed</span>
        ) : null}
      </span>
      <span className="flex shrink-0 items-center gap-1.5 text-[11.5px] font-semibold text-muted-foreground/70">
        {open ? "Hide" : "Details"}
        <ChevronDown
          className={cn("h-3 w-3 transition-transform", open && "rotate-180")}
          aria-hidden
        />
      </span>
    </button>
  );
}

function StepRow({
  step,
  working,
  expanded,
  onToggle,
}: {
  step: ActivityStep;
  working: boolean;
  expanded: boolean;
  onToggle: () => void;
}) {
  const view = toStepView(step);
  // Working expanded list dims completed steps so the live (running) one
  // reads as the focus; settled shows every step at full opacity.
  const dim = working && view.status !== "running";

  return (
    <div className={cn(dim && "opacity-60")}>
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={expanded}
        className="flex w-full min-w-0 items-center gap-2.5 rounded-md px-1.5 py-1 text-left font-mono text-[11px] hover:bg-foreground/[0.04]"
      >
        <StepGlyph status={view.status} />
        <span className="shrink-0 text-muted-foreground/60">{view.verb}</span>
        <span className="min-w-0 flex-1 truncate text-muted-foreground">
          {view.summary}
        </span>
        {view.meta && (
          <span
            className={cn(
              "shrink-0 text-muted-foreground/60",
              view.status === "error" && "text-status-attention",
            )}
          >
            {view.meta}
          </span>
        )}
      </button>
      {expanded && (
        <div className="border-l border-border/60 py-1.5 pl-3 ml-[9px] mt-0.5">
          <StepDetail step={step} />
        </div>
      )}
    </div>
  );
}

function StepDetail({ step }: { step: ActivityStep }) {
  if (step.kind === "reasoning") {
    return (
      <p className="whitespace-pre-wrap break-words text-[12.5px] italic leading-[1.6] text-muted-foreground">
        {step.text}
      </p>
    );
  }
  return <ToolCallBody item={step} />;
}

function StepGlyph({ status }: { status: StepStatus }) {
  if (status === "running") {
    return (
      <LoaderCircle
        className="h-3 w-3 shrink-0 animate-spin text-status-working"
        strokeWidth={2}
        aria-hidden
      />
    );
  }
  if (status === "error") {
    return (
      <X className="h-3 w-3 shrink-0 text-status-attention" strokeWidth={2} aria-hidden />
    );
  }
  return (
    <Check className="h-3 w-3 shrink-0 text-status-open" strokeWidth={2} aria-hidden />
  );
}
