import { Ban, Bot, Check, ChevronRight, X } from "lucide-react";
import { memo, useMemo } from "react";

import { AgentOrb } from "@/components/ui/agent-orb";
import {
  formatElapsed,
  isRunning,
  subagentGroupRollup,
} from "@/lib/agent-chat/subagents";
import type { SubagentRunItem, SubagentView } from "@/lib/agent-chat/types";
import { cn } from "@/lib/utils";
import { useUIStore } from "@/stores/ui-store";

import { TickingText } from "./TickingText";
import { formatCompactTokens } from "./WorkflowRunCard";

/**
 * Turn 2's transcript treatment: one work-log line for one uninterrupted
 * stretch of subagent work. The reducer deliberately keeps its canonical
 * per-turn spawn groups; `transcript-slots.ts` merges adjacent groups only at
 * the presentation layer and hands them here.
 *
 * This row never expands in the transcript. Its detail lives in the focused
 * Subagents panel, so 1 subagent and 40 subagents cost the same 32px line.
 */
export const SubagentWorkLogRow = memo(function SubagentWorkLogRow({
  runs,
  workspaceId,
}: {
  runs: readonly SubagentRunItem[];
  workspaceId?: string | null;
}) {
  const subagents = useMemo(
    () => runs.flatMap((run) => run.subagents),
    [runs],
  );
  const anyRunning = subagents.some(isRunning);
  const anyFailed = subagents.some((sub) => sub.status === "failed");
  const anyHalted = subagents.some(
    (sub) => sub.status === "interrupted" || sub.status === "stopped",
  );
  const selected = useUIStore((state) =>
    workspaceId
      ? state.rightPanelTabs[workspaceId] === "subagents"
      : false,
  );
  const setRightPanelTab = useUIStore((state) => state.setRightPanelTab);
  const firstRunId = runs[0]?.id;

  if (!firstRunId || subagents.length === 0) return null;

  const openPanel = () => {
    if (workspaceId) setRightPanelTab(workspaceId, "subagents");
  };

  return (
    <div
      data-testid="subagent-work-log"
      data-subagent-card={firstRunId}
      className="min-w-0"
    >
      {/* Jump requests still address canonical run ids. These zero-size DOM
          anchors let the composer find the merged visual row for any run in
          the stretch without leaking those ids into accessible text. */}
      {runs.map((run) => (
        <span key={run.id} data-subagent-run-id={run.id} className="hidden" />
      ))}

      <div className="mb-0.5 pl-0.5 font-mono text-[9px] leading-none lowercase tracking-[0.08em] text-muted-foreground/55">
        work log{anyRunning ? "" : " · settled"}
      </div>
      <button
        type="button"
        onClick={openPanel}
        disabled={!workspaceId}
        aria-pressed={selected}
        aria-label={`View ${subagents.length} subagent${subagents.length === 1 ? "" : "s"}`}
        className={cn(
          "group/work-log -mx-2 flex h-8 w-[calc(100%+1rem)] min-w-0 items-center gap-2 rounded-md px-2 text-left transition-colors",
          "hover:bg-foreground/[0.05] disabled:cursor-default",
          selected && "bg-foreground/[0.055]",
        )}
      >
        <span className="flex size-5 shrink-0 items-center justify-center">
          {anyRunning ? (
            <AgentOrb size={20} aria-hidden />
          ) : (
            <Bot
              className="size-3.5 text-muted-foreground"
              strokeWidth={1.7}
              aria-hidden
            />
          )}
        </span>
        <span className="shrink-0 text-[12px] font-semibold text-foreground/85">
          Ran {subagents.length} subagent{subagents.length === 1 ? "" : "s"}
        </span>

        {anyRunning ? (
          <>
            <span className="min-w-0 flex-1 truncate text-[11px] text-muted-foreground">
              {subagentPreview(subagents)}
            </span>
            <TickingText
              active
              className="hidden shrink-0 whitespace-nowrap font-mono text-[10px] text-muted-foreground sm:block"
              compute={(now) => liveRollupLabel(subagents, now)}
            />
          </>
        ) : (
          <>
            <TickingText
              active={false}
              className="min-w-0 flex-1 truncate font-mono text-[10px] text-muted-foreground"
              compute={(now) => settledRollupLabel(subagents, now)}
            />
            <TokenTotal subagents={subagents} />
          </>
        )}

        <span className="flex shrink-0 items-center gap-0.5 text-[10px] font-medium text-foreground/70">
          View
          <ChevronRight
            className="size-3 transition-transform group-hover/work-log:translate-x-0.5"
            strokeWidth={1.7}
            aria-hidden
          />
        </span>
        {!anyRunning && (
          <StatusGlyph failed={anyFailed} halted={anyHalted} />
        )}
      </button>
    </div>
  );
});

function StatusGlyph({ failed, halted }: { failed: boolean; halted: boolean }) {
  if (failed) {
    return (
      <X className="size-3.5 shrink-0 text-status-attention" aria-hidden />
    );
  }
  if (halted) {
    return (
      <Ban className="size-3.5 shrink-0 text-status-working/80" aria-hidden />
    );
  }
  return <Check className="size-3.5 shrink-0 text-status-open" aria-hidden />;
}

function TokenTotal({ subagents }: { subagents: readonly SubagentView[] }) {
  // Token aggregation is clock-independent; zero avoids consulting a clock
  // during render while still sharing the canonical rollup helper.
  const total = subagentGroupRollup(subagents, 0).totalTokens;
  if (total == null) return null;
  return (
    <span className="hidden shrink-0 font-mono text-[10px] text-muted-foreground sm:block">
      Σ {formatCompactTokens(total)}
    </span>
  );
}

export function subagentPreview(subagents: readonly SubagentView[]): string {
  const labels: string[] = [];
  for (const subagent of subagents) {
    const label = (subagent.name ?? subagent.agentType)?.trim();
    if (!label || labels.includes(label)) continue;
    labels.push(label);
    if (labels.length === 3) break;
  }
  return labels.length > 0 ? labels.join(" · ") : "working in parallel";
}

function liveRollupLabel(
  subagents: readonly SubagentView[],
  now: number,
): string {
  const rollup = subagentGroupRollup(subagents, now);
  const parts = [`${rollup.doneCount} done`];
  if (rollup.totalTokens != null) {
    parts.push(`Σ ${formatCompactTokens(rollup.totalTokens)}`);
  } else if (rollup.activeCount > 0) {
    parts.push(`${rollup.activeCount} active`);
  }
  return parts.join(" · ");
}

function settledRollupLabel(
  subagents: readonly SubagentView[],
  now: number,
): string {
  const rollup = subagentGroupRollup(subagents, now);
  const tools = `${rollup.toolCount} ${rollup.toolCount === 1 ? "tool" : "tools"}`;
  return rollup.elapsedMs == null
    ? tools
    : `${tools} · ${formatElapsed(rollup.elapsedMs)}`;
}

/** Kept exported for callers/tests that need the complete real-data rollup. */
export function groupRollupLabel(
  subagents: readonly SubagentView[],
  now: number,
): string {
  const rollup = subagentGroupRollup(subagents, now);
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
