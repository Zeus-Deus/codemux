import { Check, ChevronDown, LoaderCircle, X } from "lucide-react";
import { memo, useEffect, useRef, useState } from "react";

import { AgentOrb } from "@/components/ui/agent-orb";
import { turnOrbActivity } from "@/lib/agent-chat/orb-activity";
import { cn } from "@/lib/utils";

import type { ActivityStep } from "./transcript-slots";
import { ToolCallBody } from "./ToolCallBodies";
import { stepStatus, toStepView, type StepStatus } from "./activity-steps";

/**
 * Compact mechanical work log. The newest step is the default surface; older
 * contiguous work stays one click away. There is deliberately no surrounding
 * card, status banner, shimmer, counter, or settled-success header—the turn
 * fold owns completion and the final assistant answer owns the hierarchy.
 */
export const ActivityBlock = memo(function ActivityBlock({
  items,
  working,
}: {
  items: ActivityStep[];
  working: boolean;
}) {
  const [showPrevious, setShowPrevious] = useState(false);
  const [expandedStepId, setExpandedStepId] = useState<string | null>(null);
  const previousWorking = useRef(working);

  useEffect(() => {
    if (previousWorking.current && !working) {
      setShowPrevious(false);
      setExpandedStepId(null);
    }
    previousWorking.current = working;
  }, [working]);

  const visible = showPrevious ? items : items.slice(-1);
  const hiddenCount = Math.max(0, items.length - 1);
  const hiddenItems = items.slice(0, -1);
  const hiddenFailedCount = hiddenItems.filter(
    (item) => stepStatus(item) === "error",
  ).length;
  const hiddenAreTools = hiddenItems.every((item) => item.kind === "tool_call");
  const hiddenLabel = hiddenAreTools
    ? `tool call${hiddenCount === 1 ? "" : "s"}`
    : `log entr${hiddenCount === 1 ? "y" : "ies"}`;

  return (
    <div className="-mx-1 select-text px-1 py-0.5">
      <div className="space-y-px">
        {visible.map((step) => {
          const isLive = working && step.id === items[items.length - 1]?.id;
          return (
            <StepRow
              key={step.id}
              step={step}
              live={isLive}
              orbActivity={isLive ? turnOrbActivity(items) : undefined}
              expanded={expandedStepId === step.id}
              onToggle={() =>
                setExpandedStepId((current) =>
                  current === step.id ? null : step.id,
                )
              }
            />
          );
        })}
      </div>

      {hiddenCount > 0 && (
        <button
          type="button"
          aria-expanded={showPrevious}
          onClick={() => setShowPrevious((current) => !current)}
          className="flex w-full items-center gap-1.5 rounded-md px-0.5 py-0.5 text-left text-[12px] leading-5 text-foreground/80 transition-colors hover:bg-foreground/[0.035] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/70"
        >
          <span className="flex size-5 shrink-0 items-center justify-center text-muted-foreground/65">
            <ChevronDown
              className={cn(
                "size-3.5 transition-transform duration-150",
                showPrevious && "rotate-180",
              )}
              aria-hidden
            />
          </span>
          <span className="font-medium">
            {showPrevious
              ? "Show fewer work entries"
              : `+${hiddenCount} previous ${hiddenLabel}`}
          </span>
          {hiddenFailedCount > 0 && !showPrevious ? (
            <span className="text-status-attention">
              · {hiddenFailedCount} failed
            </span>
          ) : null}
        </button>
      )}
    </div>
  );
});

function StepRow({
  step,
  live,
  orbActivity,
  expanded,
  onToggle,
}: {
  step: ActivityStep;
  live: boolean;
  orbActivity?: ReturnType<typeof turnOrbActivity>;
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
        className="flex w-full min-w-0 items-center gap-1.5 rounded-md px-0.5 py-0.5 text-left text-[12px] leading-5 transition-colors hover:bg-foreground/[0.035] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/70"
      >
        <span className="flex size-5 shrink-0 items-center justify-center">
          {live ? (
            <AgentOrb size={20} {...(orbActivity ?? {})} aria-hidden />
          ) : (
            <StepGlyph status={view.status} />
          )}
        </span>
        <span className="shrink-0 text-muted-foreground/65">{view.verb}</span>
        <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-muted-foreground">
          {view.summary}
        </span>
        {view.meta && !live ? (
          <span
            className={cn(
              "shrink-0 font-mono text-[10px] text-muted-foreground/55",
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
            <p className="whitespace-pre-wrap break-words text-[13px] italic leading-[1.6] text-muted-foreground">
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
