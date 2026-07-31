import { CheckCircle2, LoaderCircle } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { basename } from "@/lib/path";
import { openEditorTab } from "@/lib/open-editor-tab";
import {
  describeToolCall,
  isRunning,
  recentToolCalls,
  subagentActivityLine,
} from "@/lib/agent-chat/subagents";
import { subagentFindingBadge } from "@/lib/agent-chat/workflows";
import type { SubagentView } from "@/lib/agent-chat/types";
import { cn } from "@/lib/utils";
import type { WorkspaceSnapshot } from "@/tauri/types";

import { findingTone, workflowAgentTone } from "./workflow-tone";

/** A label "looks like a file path" when it has no spaces and ends in a
 *  short extension — good enough to gate the "Open {file}" affordance
 *  without a real repo lookup. Subagent names/labels that aren't file
 *  paths (e.g. "Auth auditor") simply don't get the button, rather than
 *  faking a broken open action. */
function looksLikeFilePath(label: string): boolean {
  if (!label || label.includes(" ") || label.length > 200) return false;
  return /\.[a-zA-Z0-9]{1,10}$/.test(label);
}

/** Best-available prompt text for the drill-in: the subagent's own
 *  sub-transcript carries no dedicated prompt field (see
 *  `SubagentView`/`SubagentSnapshot` — the Task-tool prompt lives only on
 *  the PARENT thread's tool_call item), so this falls back through the
 *  first non-empty user/assistant text actually observed in the
 *  sub-transcript, then the subagent's name/activity. */
function derivePrompt(sub: SubagentView): string {
  for (const item of sub.items) {
    if (
      (item.kind === "user_message" || item.kind === "assistant_message") &&
      item.text.trim().length > 0
    ) {
      return item.text;
    }
  }
  return sub.name ?? sub.activity ?? "No prompt captured.";
}

interface Props {
  agent: SubagentView;
  phaseIndex: number;
  phaseTitle: string;
  agentIndex: number;
  agentsInPhase: number;
  workspace: WorkspaceSnapshot;
}

/**
 * Agent-detail level of the Orchestration panel (design "Drill into any
 * agent's work"): header strip, then Prompt / Recent tool calls / Result
 * sections. Restart is a stubbed affordance (no restart backend yet);
 * "Open {file}" only appears when the agent's label looks like a real
 * repo file path, reusing the same `openEditorTab` helper the file tree
 * uses rather than faking a broken action for non-file agents.
 */
export function WorkflowAgentDetail({
  agent,
  phaseIndex,
  phaseTitle,
  agentIndex,
  agentsInPhase,
  workspace,
}: Props) {
  const badge = subagentFindingBadge(agent);
  const tone = badge ? findingTone(badge.tone) : workflowAgentTone(agent.status);
  const running = isRunning(agent);
  const label = agent.name ?? agent.activity ?? "Subagent";
  const tools = recentToolCalls(agent, 6);
  const canOpenFile = looksLikeFilePath(label);

  return (
    <div
      className="flex min-w-0 max-w-full flex-col gap-4 overflow-hidden p-3"
      data-testid="workflow-agent-detail"
    >
      <div
        className={cn(
          "flex items-center gap-2.5 rounded-[9px] border px-2.5 py-2.5",
          tone.softBg,
          tone.border,
        )}
      >
        {running ? (
          <LoaderCircle className={cn("h-4 w-4 shrink-0 animate-spin", tone.text)} strokeWidth={1.8} aria-hidden />
        ) : (
          <CheckCircle2 className={cn("h-4 w-4 shrink-0", tone.text)} strokeWidth={1.8} aria-hidden />
        )}
        <div className="min-w-0 flex-1">
          <div className="truncate font-mono text-[12px] font-semibold text-foreground">{label}</div>
          <div className="truncate text-[11px] text-muted-foreground">
            Phase {phaseIndex} · {phaseTitle} · agent {agentIndex} of {agentsInPhase}
            {agent.model ? ` · ${agent.model}` : ""}
          </div>
        </div>
        {badge && (
          <span
            className={cn(
              "shrink-0 rounded-[5px] px-2 py-0.5 text-[10px] font-bold uppercase",
              tone.chipBg,
            )}
          >
            {badge.label}
          </span>
        )}
      </div>

      <section>
        <SectionLabel>Prompt</SectionLabel>
        <div className="break-words rounded-[9px] border border-border/60 bg-muted/30 px-2.5 py-2.5 text-[12px] leading-[1.55] text-foreground/90">
          {derivePrompt(agent)}
        </div>
      </section>

      <section>
        <SectionLabel>Recent tool calls</SectionLabel>
        {tools.length === 0 ? (
          <div className="px-1 text-[11px] text-muted-foreground">No tool activity yet.</div>
        ) : (
          <div className="flex flex-col gap-0.5">
            {tools.map((tool) => {
              const d = describeToolCall(tool);
              return (
                <div
                  key={tool.id}
                  className="flex min-w-0 items-center gap-2.5 px-0.5 py-1 font-mono text-[11px]"
                >
                  <span className="shrink-0 text-muted-foreground/70">{d.verb}</span>
                  <span className="min-w-0 flex-1 truncate text-foreground/80">{d.target}</span>
                  {d.meta && <span className="shrink-0 text-muted-foreground/70">{d.meta}</span>}
                </div>
              );
            })}
          </div>
        )}
      </section>

      <section>
        <SectionLabel>Result</SectionLabel>
        {running ? (
          <div className="rounded-[9px] border border-border/60 bg-muted/30 px-2.5 py-2.5 text-[12px] text-muted-foreground">
            <span className="shimmer font-mono text-[12px]">{subagentActivityLine(agent)}</span>
          </div>
        ) : (
          <div
            className={cn(
              "break-words rounded-[9px] border px-2.5 py-2.5 text-[12px] leading-[1.55] text-foreground/90",
              badge?.tone === "red" ? cn(tone.softBg, tone.border) : "border-border/60 bg-muted/30",
            )}
          >
            {agent.resultText ?? "No result yet."}
          </div>
        )}
      </section>

      <div className="flex gap-2 pt-1">
        <Tooltip>
          <TooltipTrigger asChild>
            <Button variant="outline" size="sm" className="text-[12px]" disabled>
              Restart agent
            </Button>
          </TooltipTrigger>
          <TooltipContent side="top">Restarting isn't supported yet.</TooltipContent>
        </Tooltip>
        {canOpenFile && (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="text-[12px] text-muted-foreground hover:text-foreground"
            onClick={() => {
              openEditorTab(workspace.workspace_id, workspace.tabs, label).catch(console.error);
            }}
          >
            Open {basename(label)}
          </Button>
        )}
      </div>
    </div>
  );
}

function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <div className="mb-1.5 font-mono text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
      {children}
    </div>
  );
}
