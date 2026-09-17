import { ChevronLeft, ChevronRight } from "lucide-react";

import {
  isRunning,
  statusTone,
  subagentStatusLabel,
} from "@/lib/agent-chat/subagents";
import type { SubagentView } from "@/lib/agent-chat/types";
import { cn } from "@/lib/utils";
import { PanelHeader } from "@/components/ui/panel-header";

/**
 * Breadcrumb sub-header shown while a subagent drill-in is open (design
 * "Enter a subagent to watch it work"). Replaces the orchestrator
 * sub-header: a back affordance to the orchestrator, the subagent's
 * ordinal glyph + name, a mono model chip, and a right-aligned
 * tone-tinted status. Esc / the back button return to the orchestrator
 * (handled by the pane).
 */
export function SubagentBreadcrumb({
  subagent,
  ordinal,
  onBack,
}: {
  subagent: SubagentView;
  ordinal: number;
  onBack: () => void;
}) {
  const tone = statusTone(subagent.status);
  const name = subagent.name ?? subagent.agentType ?? "Subagent";
  const label = subagentStatusLabel(subagent);

  return (
    <PanelHeader
      data-testid="subagent-breadcrumb"
      className={cn(
        "gap-2.5 bg-card px-3",
        // Under the floating titlebar (a lone chat tab), start below its
        // 40px band. Otherwise the band's tabs and actions cover this row.
        "[[data-under-titlebar=true]_&]:mt-10",
      )}
    >
      <button
        type="button"
        onClick={onBack}
        className="flex h-[26px] shrink-0 items-center gap-1.5 rounded-md px-2 text-body-sm font-semibold text-muted-foreground hover:bg-surface-2 hover:text-foreground"
      >
        <ChevronLeft className="size-3.5" aria-hidden />
        Orchestrator
      </button>
      <ChevronRight
        className="size-3 shrink-0 text-muted-foreground/70"
        aria-hidden
      />
      <span className="flex min-w-0 items-center gap-2 text-body font-semibold text-foreground">
        <span
          className={cn(
            "flex h-[19px] w-[19px] shrink-0 items-center justify-center rounded-md font-mono text-caption",
            tone.chipBg,
          )}
        >
          {ordinal || "•"}
        </span>
        <span className="truncate">{name}</span>
      </span>
      {subagent.model && (
        <span className="shrink-0 rounded-sm bg-surface-2 px-1.5 py-0.5 font-mono text-label text-muted-foreground">
          {subagent.model}
        </span>
      )}
      <span
        className={cn(
          "ml-auto flex shrink-0 items-center gap-1.5 text-label font-semibold",
          tone.text,
        )}
      >
        {isRunning(subagent) && (
          <span
            className={cn(
              "cm-blink size-1.5 rounded-full",
              subagent.status === "running"
                ? "bg-status-working"
                : "bg-muted-foreground",
            )}
            aria-hidden
          />
        )}
        {label}
      </span>
    </PanelHeader>
  );
}
