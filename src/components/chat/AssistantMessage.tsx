import { memo } from "react";

import type { AssistantMessageItem } from "@/lib/agent-chat/types";

import { ChatMarkdown } from "./ChatMarkdown";
import { StreamingIndicator } from "./streaming-indicator";

/**
 * Assistant prose (design D4). No bubble, no avatar (the avatar lives in
 * the turn gutter, drawn once by MessageList) — just the streaming
 * markdown at 14px / 1.625 line-height. All element styling lives in
 * `ChatMarkdown` so the plan renderer and reasoning body share one prose
 * treatment. The pulsing caret trails a streaming row.
 */
export const AssistantMessage = memo(function AssistantMessage({
  item,
  workspaceId,
  cwd,
}: {
  item: AssistantMessageItem;
  workspaceId?: string | null;
  cwd?: string | null;
}) {
  const showIndicator = item.streaming && item.text.length === 0;

  return (
    <div className="text-sm leading-relaxed text-foreground break-words">
      <ChatMarkdown
        streaming={item.streaming}
        workspaceId={workspaceId}
        cwd={cwd}
      >
        {item.text}
      </ChatMarkdown>
      {item.streaming && !showIndicator && (
        <StreamingIndicator className="ml-0.5" />
      )}
      {showIndicator && <StreamingIndicator />}
    </div>
  );
});
