import { memo } from "react";

import type { AssistantMessageItem } from "@/lib/agent-chat/types";

import { ChatMarkdown } from "./ChatMarkdown";
import { StreamingIndicator } from "./streaming-indicator";

/**
 * Assistant prose (design D4). No bubble, no avatar (the avatar lives in
 * the turn gutter, drawn once by MessageList) — just the streaming
 * markdown at 13.5px / 1.62 line-height. All element styling lives in
 * `ChatMarkdown` so the plan renderer and reasoning body share one prose
 * treatment. The pulsing caret trails a streaming row.
 */
export const AssistantMessage = memo(function AssistantMessage({
  item,
}: {
  item: AssistantMessageItem;
}) {
  const showIndicator = item.streaming && item.text.length === 0;

  return (
    <div className="text-[13.5px] leading-[1.62] text-foreground break-words">
      <ChatMarkdown>{item.text}</ChatMarkdown>
      {item.streaming && !showIndicator && (
        <StreamingIndicator className="ml-0.5" />
      )}
      {showIndicator && <StreamingIndicator />}
    </div>
  );
});
