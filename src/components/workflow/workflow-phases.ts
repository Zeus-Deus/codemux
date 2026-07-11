import type { SubagentView, WorkflowRunItem } from "@/lib/agent-chat/types";

/** One phase row for the panel, merging live phases with any planned
 *  phase the workflow hasn't attributed a live phase to yet. */
export interface CombinedPhase {
  title: string;
  detail: string | null;
  agents: SubagentView[];
}

/**
 * Combine `run.phases` (live, carries agents — including any synthesized
 * trailing "Run" phase for subagents whose `phase` hint didn't match a
 * planned title) with any `plannedPhases` entry that has no matching
 * live phase yet. In practice the reducer sets both lists together, so
 * this rarely finds anything to append — it exists so a planned phase is
 * never silently dropped before the workflow attributes agents to it.
 */
export function combinePhases(run: WorkflowRunItem): CombinedPhase[] {
  const live = run.phases;
  const liveTitles = new Set(live.map((p) => p.title));
  const queued: CombinedPhase[] = run.plannedPhases
    .filter((p) => !liveTitles.has(p.title))
    .map((p) => ({ title: p.title, detail: p.detail, agents: [] }));
  return [...live, ...queued];
}

export interface AgentContext {
  agent: SubagentView;
  /** 1-based phase ordinal within the combined phase list. */
  phaseIndex: number;
  phaseTitle: string;
  /** 1-based ordinal of the agent within its phase. */
  agentIndex: number;
  agentsInPhase: number;
}

/** Locate a subagent by id anywhere in the run's combined phases, along
 *  with the phase/ordinal context the agent-detail header needs. */
export function findAgentContext(run: WorkflowRunItem, agentId: string): AgentContext | null {
  const phases = combinePhases(run);
  for (let pi = 0; pi < phases.length; pi++) {
    const idx = phases[pi].agents.findIndex((a) => a.id === agentId);
    if (idx >= 0) {
      return {
        agent: phases[pi].agents[idx],
        phaseIndex: pi + 1,
        phaseTitle: phases[pi].title,
        agentIndex: idx + 1,
        agentsInPhase: phases[pi].agents.length,
      };
    }
  }
  return null;
}
