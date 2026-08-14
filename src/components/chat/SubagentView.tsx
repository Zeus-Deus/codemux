import { Lock } from "lucide-react";
import { useMemo } from "react";

import { AgentOrb } from "@/components/ui/agent-orb";
import { subagentOrbActivity } from "@/lib/agent-chat/orb-activity";
import {
  isRunning,
  statusTone,
  subagentActivityLine,
} from "@/lib/agent-chat/subagents";
import type {
  ChatViewItem,
  PermissionRequestItem,
  SubagentView as SubagentViewModel,
  ToolCallItem,
} from "@/lib/agent-chat/types";
import { cn } from "@/lib/utils";

import { ActivityBlock } from "./ActivityBlock";
import { AssistantMessage } from "./AssistantMessage";
import { ReasoningBlock } from "./ReasoningBlock";
import { isTaskSummaryTool, TaskSummaryCard } from "./TaskSummaryCard";
import { ToolCallCard } from "./ToolCallCard";
import { UserMessage } from "./UserMessage";
import { buildTranscriptSlots } from "./transcript-slots";
import { CHAT_COLUMN } from "./chat-column";
import { ChatFileLinkContext } from "./chat-file-link-context";

const NOOP = () => {};

/**
 * Read-only drill-in for one subagent (design "Enter a subagent to watch
 * it work"). A tone-tinted banner marks the view read-only, then the
 * subagent's own sub-transcript renders through the SAME renderers the
 * main transcript uses (reasoning / tool / diff / task cards, markdown),
 * folded by the shared `buildTranscriptSlots`. A live shimmer tail shows
 * the current step while the subagent is still running.
 *
 * `requests` are the parent-thread approval requests tagged with this
 * subagent's id (locked decision 4) — merged in by seq so a bubbled
 * approval also shows here.
 */
export function SubagentView({
  subagent,
  requests = [],
  workspaceId,
  cwd,
}: {
  subagent: SubagentViewModel;
  requests?: PermissionRequestItem[];
  workspaceId?: string | null;
  cwd?: string | null;
}) {
  const tone = statusTone(subagent.status);
  const running = isRunning(subagent);
  const name = subagent.name ?? subagent.agentType ?? "subagent";

  // Merge the sub-transcript with any bubbled approval requests, ordered
  // by seq, then fold through the shared slot builder.
  const slots = useMemo(() => {
    const merged: ChatViewItem[] = [...subagent.items, ...requests].sort(
      (a, b) => a.seq - b.seq || a.id.localeCompare(b.id),
    );
    return buildTranscriptSlots(merged);
  }, [subagent.items, requests]);

  return (
    <ChatFileLinkContext.Provider value={{ workspaceId, cwd }}>
      <div className={cn(CHAT_COLUMN, "pb-[30px] pt-[26px]")}>
      {/* Read-only banner */}
      <div
        className={cn(
          "mb-4 flex items-center gap-2.5 rounded-[9px] border px-3 py-2.5 text-[12px] text-muted-foreground",
          tone.softBg,
          tone.border,
        )}
      >
        <Lock className={cn("h-3.5 w-3.5 shrink-0", tone.text)} strokeWidth={1.5} aria-hidden />
        <span>
          Read-only view of the <b className="text-foreground">{name}</b>{" "}
          subagent. To change direction, message the orchestrator.
        </span>
      </div>

      {/* Sub-transcript through the existing renderers. Folded via the
          shared slot builder with `streaming=false`, so contiguous
          reasoning/tool runs roll up into a SETTLED Activity block (#124);
          the drill-in's own live tail below handles the running indicator. */}
      <div className="flex flex-col">
        {slots.map((slot) => (
          <div key={slot.key} className={slot.turnStart ? "mt-4" : "mt-3"}>
            {slot.body.kind === "activity" ? (
              <ActivityBlock items={slot.body.items} working={slot.body.working} />
            ) : slot.body.kind === "turn_fold" ? (
              <div className="border-b border-border/60 pb-2 text-xs text-muted-foreground">
                {slot.body.label}
              </div>
            ) : slot.body.kind === "subagent_stretch" ? (
              <div className="h-8 text-[11px] text-muted-foreground">
                Ran {slot.body.runs.reduce(
                  (count, run) => count + run.subagents.length,
                  0,
                )}{" "}
                nested subagents
              </div>
            ) : (
              <SubItem item={slot.body.item} workspaceId={workspaceId} cwd={cwd} />
            )}
          </div>
        ))}

        {/* Live tail while running */}
        {running && (
          <div
            className="mt-3 flex items-center gap-1.5 px-1 py-1 text-[12px] leading-5 text-muted-foreground"
          >
            <span className="flex h-5 w-5 shrink-0 items-center justify-center">
              <AgentOrb
                size={20}
                {...subagentOrbActivity(subagent)}
                aria-hidden
              />
            </span>
            <span className="min-w-0 truncate font-mono text-[11px]">
              {subagentActivityLine(subagent)}
            </span>
          </div>
        )}
      </div>
      </div>
    </ChatFileLinkContext.Provider>
  );
}

/** Render one sub-transcript item through the existing renderers.
 *  Tool cards are inert (read-only): approvals happen in the parent. */
function SubItem({
  item,
  workspaceId,
  cwd,
}: {
  item: ChatViewItem;
  workspaceId?: string | null;
  cwd?: string | null;
}) {
  switch (item.kind) {
    case "user_message":
      return <UserMessage item={item} />;
    case "assistant_message":
      return <AssistantMessage item={item} workspaceId={workspaceId} cwd={cwd} />;
    case "reasoning":
      // The live tail below owns the run's orb, so a still-streaming thought
      // keeps its settled glyph here instead of animating a second one.
      return <ReasoningBlock item={item} live={false} />;
    case "tool_call":
      return isTaskSummaryTool(item as ToolCallItem) ? (
        <TaskSummaryCard item={item as ToolCallItem} />
      ) : (
        <ToolCallCard item={item as ToolCallItem} approval={null} onDecide={NOOP} />
      );
    case "permission_request":
      return (
        <div className="rounded-[9px] border border-border/60 bg-muted/40 px-3 py-2 text-xs text-muted-foreground">
          {item.resolution.state === "failed"
            ? item.resolution.message
            : item.resolution.state === "pending"
              ? "Approval requested — respond in the orchestrator."
              : "Approval handled in the orchestrator."}
        </div>
      );
    default:
      // turn_ended / subagent_run (nested grandchildren) render nothing in
      // the v1 drill-in.
      return null;
  }
}
