/**
 * Subagents pane — the focused thread's spawn groups, in the deck.
 *
 * This is a *placement*, not a new presentation: it renders the same
 * `SubagentsCard` the transcript renders, from the same
 * `subagentRunItems(messages)` derivation, so a group looks and reads
 * identically whether you meet it inline or in the panel. Entering a
 * subagent still belongs to the chat pane (the drill-in replaces the
 * transcript), so the row's Enter routes through a one-shot ui-store
 * request that `AgentChatPane` consumes.
 */
import { useMemo } from "react";

import { SubagentsCard } from "@/components/chat/SubagentsCard";
import { subagentRunItems } from "@/lib/agent-chat/subagents";
import type { ChatViewItem } from "@/lib/agent-chat/types";
import { useUIStore } from "@/stores/ui-store";

export function SubagentsPane({
  threadId,
  messages,
}: {
  threadId: string | null;
  messages: ChatViewItem[];
}) {
  const requestEnterSubagent = useUIStore((s) => s.requestEnterSubagent);
  const runs = useMemo(() => subagentRunItems(messages), [messages]);

  if (runs.length === 0) {
    return (
      <div className="flex h-full items-center justify-center px-6 text-center">
        <p className="text-[11px] text-muted-foreground/70">
          No subagents in this thread yet.
        </p>
      </div>
    );
  }

  return (
    <div
      data-testid="subagents-pane"
      className="flex h-full min-h-0 flex-col gap-3 overflow-y-auto p-3"
    >
      {runs.map((run) => (
        <SubagentsCard
          key={run.id}
          item={run}
          onEnter={(subagentId) => {
            if (threadId) requestEnterSubagent(threadId, subagentId);
          }}
        />
      ))}
    </div>
  );
}
