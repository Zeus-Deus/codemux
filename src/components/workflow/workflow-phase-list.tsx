import { useState } from "react";
import { CheckCircle2, ChevronRight, LoaderCircle } from "lucide-react";

import { AgentOrb } from "@/components/ui/agent-orb";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { formatCompactTokens } from "@/components/chat/WorkflowRunCard";
import { formatElapsed } from "@/lib/agent-chat/subagents";
import { subagentFindingBadge, workflowPhaseStats, workflowPhaseStatus } from "@/lib/agent-chat/workflows";
import type { SubagentView, WorkflowRunItem, WorkflowRunStatus } from "@/lib/agent-chat/types";
import { cn } from "@/lib/utils";

import { combinePhases, type CombinedPhase } from "./workflow-phases";
import { findingTone, workflowAgentTone, workflowPhaseTone } from "./workflow-tone";

type AgentFilter = "all" | "running" | "issues";
const AGENT_FILTERS: readonly AgentFilter[] = ["all", "running", "issues"];

/**
 * Phases level of the Orchestration panel — one card per phase (planned
 * or live), in order. Phases carrying agents expand into a filterable
 * agent list; row click drills into `onSelectAgent`.
 */
export function WorkflowPhaseList({
  run,
  now,
  onSelectAgent,
}: {
  run: WorkflowRunItem;
  now: number;
  onSelectAgent: (agentId: string) => void;
}) {
  const phases = combinePhases(run);

  const [openTitle, setOpenTitle] = useState<string | null>(() => {
    const running = phases.find(
      (p) => p.agents.length > 0 && workflowPhaseStatus(p, run.status) === "running",
    );
    return running?.title ?? null;
  });

  if (phases.length === 0) {
    return (
      <div className="px-3 py-8 text-center text-[12px] text-muted-foreground">
        No phases yet.
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-2 p-2.5">
      {phases.map((phase, i) => (
        <PhaseCard
          key={phase.title}
          index={i + 1}
          phase={phase}
          runStatus={run.status}
          now={now}
          open={openTitle === phase.title}
          onToggle={() => setOpenTitle((cur) => (cur === phase.title ? null : phase.title))}
          onSelectAgent={onSelectAgent}
        />
      ))}
    </div>
  );
}

function PhaseCard({
  index,
  phase,
  runStatus,
  now,
  open,
  onToggle,
  onSelectAgent,
}: {
  index: number;
  phase: CombinedPhase;
  runStatus: WorkflowRunStatus;
  now: number;
  open: boolean;
  onToggle: () => void;
  onSelectAgent: (agentId: string) => void;
}) {
  const status = workflowPhaseStatus(phase, runStatus);
  const stats = workflowPhaseStats(phase, now);
  const tone = workflowPhaseTone(status);
  const hasAgents = phase.agents.length > 0;

  const subLine =
    status === "running"
      ? `${stats.done} done · ${stats.running} running`
      : (phase.detail ??
        (status === "pending"
          ? "queued"
          : status === "failed"
            ? `${stats.failed} failed`
            : `${stats.done} done`));

  const agentCountLabel = stats.total > 0 ? `${stats.total} agent${stats.total === 1 ? "" : "s"}` : "—";
  const metaLabel =
    stats.total > 0 ? `${formatCompactTokens(stats.tokens)} · ${formatElapsed(stats.elapsedMs)}` : "queued";

  const statusIcon = (
    <span className="flex h-[22px] w-[22px] shrink-0 items-center justify-center">
      {status === "running" ? (
        // Neutral: a phase is many agents, so it carries no single activity.
        <AgentOrb size={20} aria-hidden />
      ) : status === "pending" ? (
        <span className="h-[9px] w-[9px] rounded-full border-[1.6px] border-muted-foreground" aria-hidden />
      ) : (
        <CheckCircle2 className={cn("h-[18px] w-[18px]", tone.text)} strokeWidth={1.8} aria-hidden />
      )}
    </span>
  );

  const headerBody = (
    <>
      {statusIcon}
      <div className="min-w-0 flex-1">
        <div className="text-[13px] font-bold text-foreground">
          {index} · {phase.title}
        </div>
        <div className="truncate text-[11px] text-muted-foreground">{subLine}</div>
      </div>
      <div className="shrink-0 text-right">
        <div className="font-mono text-[11px] text-foreground/80">{agentCountLabel}</div>
        <div className="font-mono text-[10px] text-muted-foreground">{metaLabel}</div>
      </div>
    </>
  );

  if (!hasAgents) {
    return (
      <div
        className="overflow-hidden rounded-[11px] border border-border/60 bg-foreground/[0.015]"
        data-testid="workflow-phase-row"
      >
        <div className="flex items-center gap-2.5 px-3 py-2.5">{headerBody}</div>
      </div>
    );
  }

  return (
    <Collapsible
      open={open}
      onOpenChange={onToggle}
      className="overflow-hidden rounded-[11px] border border-border/60 bg-foreground/[0.015]"
      data-testid="workflow-phase-row"
    >
      <CollapsibleTrigger asChild>
        <div
          role="button"
          tabIndex={0}
          className="flex cursor-pointer items-center gap-2.5 px-3 py-2.5 hover:bg-foreground/[0.03]"
        >
          {headerBody}
        </div>
      </CollapsibleTrigger>
      <CollapsibleContent>
        <PhaseAgents phase={phase} onSelectAgent={onSelectAgent} />
      </CollapsibleContent>
    </Collapsible>
  );
}

function PhaseAgents({
  phase,
  onSelectAgent,
}: {
  phase: CombinedPhase;
  onSelectAgent: (agentId: string) => void;
}) {
  const [filter, setFilter] = useState<AgentFilter>("all");

  const filteredAgents = phase.agents.filter((agent) => {
    if (filter === "running") return agent.status === "running";
    if (filter === "issues") return subagentFindingBadge(agent)?.tone === "red";
    return true;
  });

  return (
    <div className="border-t border-border/60 p-1.5">
      <div className="flex items-center gap-1.5 px-1.5 py-1">
        <span className="font-mono text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
          Agents
        </span>
        <div className="ml-auto flex gap-1">
          {AGENT_FILTERS.map((f) => (
            <button
              key={f}
              type="button"
              onClick={(e) => {
                e.stopPropagation();
                setFilter(f);
              }}
              className={cn(
                "rounded-full px-2 py-0.5 text-[10px] font-semibold",
                filter === f
                  ? "bg-foreground/[0.12] text-foreground"
                  : "text-muted-foreground hover:text-foreground/80",
              )}
            >
              {f}
            </button>
          ))}
        </div>
      </div>
      {filteredAgents.length === 0 ? (
        <div className="px-2 py-2 text-[11px] text-muted-foreground">No agents match.</div>
      ) : (
        filteredAgents.map((agent) => (
          <AgentRow key={agent.id} agent={agent} onSelect={() => onSelectAgent(agent.id)} />
        ))
      )}
    </div>
  );
}

function AgentRow({ agent, onSelect }: { agent: SubagentView; onSelect: () => void }) {
  const tone = workflowAgentTone(agent.status);
  const badge = subagentFindingBadge(agent);
  const label = agent.name ?? agent.activity ?? "Subagent";

  return (
    <div
      role="button"
      tabIndex={0}
      onClick={onSelect}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onSelect();
        }
      }}
      className="flex cursor-pointer items-center gap-2 rounded-[8px] px-1.5 py-1.5 hover:bg-foreground/[0.05]"
      data-testid="workflow-agent-row"
    >
      <span className="flex h-4 w-4 shrink-0 items-center justify-center">
        {agent.status === "running" ? (
          <LoaderCircle className={cn("h-3.5 w-3.5 animate-spin", tone.text)} strokeWidth={2} aria-hidden />
        ) : agent.status === "pending" ? (
          <span className="h-[7px] w-[7px] rounded-full border-[1.4px] border-muted-foreground" aria-hidden />
        ) : (
          <CheckCircle2 className={cn("h-3.5 w-3.5", tone.text)} strokeWidth={2} aria-hidden />
        )}
      </span>
      <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-muted-foreground">{label}</span>
      {badge && (
        <span
          className={cn(
            "shrink-0 rounded-[4px] px-1.5 py-0.5 text-[10px] font-semibold",
            findingTone(badge.tone).chipBg,
          )}
        >
          {badge.label}
        </span>
      )}
      <ChevronRight className="h-2.5 w-2.5 shrink-0 text-muted-foreground" strokeWidth={1.7} aria-hidden />
    </div>
  );
}
