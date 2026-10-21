import { Bot, ChevronRight, Lightbulb, Minimize2, PenLine, type LucideIcon } from "lucide-react";
import { useMemo } from "react";

import { Trace, TraceIcon, TraceLine, TraceTitle } from "@/components/ui/trace";
import { countRunningSubagents } from "@/lib/agent-chat/subagents";
import type { ChatViewItem } from "@/lib/agent-chat/types";
import { cn } from "@/lib/utils";
import { useUIStore } from "@/stores/ui-store";

import { toolIcon } from "./tool-visuals";

/**
 * Tail "working" marker — the last row inside the scroller while a turn is
 * in flight and no other row shows live work (right after send, after the
 * agent's narration, while it waits on background tasks). Drawn as one
 * quiet Trace line so it reads like the work log it hands over to: an icon
 * and a softly sweeping label. Gating lives in `shouldShowThinkingIndicator`.
 *
 * It carries no clock: the turn's live header ("Working for …") is the
 * thread's one elapsed time.
 *
 * While the agent is waiting on background tasks, the line becomes a
 * button that opens the Subagents panel, which lists those same tasks
 * (subagents and background shell jobs). The line names the work, so it
 * links to it.
 */
export function StreamingMarker({
  messages,
  workspaceId,
  compacting = false,
}: {
  messages: ChatViewItem[];
  workspaceId?: string | null;
  compacting?: boolean;
}) {
  const { label, waiting, icon: Icon } = useMemo(
    () =>
      compacting
        ? { label: "Summarizing context…", waiting: 0, icon: Minimize2 }
        : deriveStreamingStatus(messages),
    [compacting, messages],
  );

  const line = (
    <TraceLine status="running">
      <TraceIcon>
        <Icon />
      </TraceIcon>
      <TraceTitle>{label}</TraceTitle>
    </TraceLine>
  );

  return (
    <div
      className="py-0.5"
      role="status"
      aria-label={compacting ? "Agent is summarizing context" : "Agent is working"}
    >
      <Trace>
        {waiting > 0 && workspaceId ? (
          <WaitingLine waiting={waiting} workspaceId={workspaceId}>
            {line}
          </WaitingLine>
        ) : (
          <div>{line}</div>
        )}
      </Trace>
    </div>
  );
}

/** The "Waiting on N background tasks…" line as a link to the Subagents
 *  panel, with a quiet `View ›` cue like the agents row in the transcript. */
function WaitingLine({
  waiting,
  workspaceId,
  children,
}: {
  waiting: number;
  workspaceId: string;
  children: React.ReactNode;
}) {
  const selected = useUIStore(
    (state) => state.rightPanelTabs[workspaceId] === "subagents",
  );
  const setRightPanelTab = useUIStore((state) => state.setRightPanelTab);
  return (
    <button
      type="button"
      onClick={() => setRightPanelTab(workspaceId, "subagents")}
      aria-pressed={selected}
      aria-label={`View ${waiting} background task${waiting === 1 ? "" : "s"}`}
      className={cn("group/waiting flex items-center gap-1.5", selected && "text-foreground")}
    >
      {children}
      <span className="flex shrink-0 items-center gap-0.5 text-caption font-medium text-muted-foreground/70 transition-colors duration-150 group-hover/waiting:text-foreground/80">
        View
        <ChevronRight
          className="size-3 transition-transform duration-150 group-hover/waiting:translate-x-0.5"
          aria-hidden
        />
      </span>
    </button>
  );
}

/** Status line derived from the transcript tail. Running tool → "Running
 *  <tool>…", streaming prose → "Writing…", anything else the agent is
 *  between steps → "Thinking…". A settled tail while background tasks
 *  (delegated subagents or background shell jobs) are still running is the
 *  agent waiting on them → "Waiting on N background tasks…". `waiting` is
 *  the number of background tasks being waited on, and non-zero only for a
 *  "Waiting on…" label. */
export function deriveStreamingStatus(messages: ChatViewItem[]): {
  label: string;
  waiting: number;
  icon: LucideIcon;
} {
  const thinking = { label: "Thinking…", waiting: 0, icon: Lightbulb };
  const last = messages[messages.length - 1];
  if (!last) return thinking;
  switch (last.kind) {
    case "tool_call":
      if (last.status === "running") {
        return { label: `Running ${last.tool_name}…`, waiting: 0, icon: toolIcon(last.tool_name) };
      }
      break;
    case "assistant_message":
      if (last.streaming) return { label: "Writing…", waiting: 0, icon: PenLine };
      break;
    default:
      break;
  }
  const waiting = countRunningSubagents(messages, true);
  if (waiting === 1) return { label: "Waiting on a background task…", waiting, icon: Bot };
  if (waiting > 1) {
    return { label: `Waiting on ${waiting} background tasks…`, waiting, icon: Bot };
  }
  return thinking;
}
