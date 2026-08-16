import { memo } from "react";

import type { AssistantMessageItem } from "@/lib/agent-chat/types";

import { ChatMarkdown } from "./ChatMarkdown";

/**
 * Assistant prose (design D4). No bubble, no avatar (the avatar lives in
 * the turn gutter, drawn once by MessageList) — just the streaming
 * markdown at 14px / 1.625 line-height. All element styling lives in
 * `ChatMarkdown` so the plan renderer and reasoning body share one prose
 * treatment. Live progress is communicated by the transcript's dedicated
 * streaming marker rather than extra chrome appended to the prose.
 */
export const AssistantMessage = memo(function AssistantMessage({
  item,
  workspaceId,
  cwd,
  referenceCwd,
  referencePaths,
}: {
  item: AssistantMessageItem;
  workspaceId?: string | null;
  cwd?: string | null;
  referenceCwd?: string | null;
  referencePaths?: readonly string[];
}) {
  return (
    <div className="conversation-text leading-relaxed text-foreground break-words">
      <ChatMarkdown
        streaming={item.streaming}
        workspaceId={workspaceId}
        cwd={referenceCwd ?? cwd}
        referencePaths={referencePaths}
      >
        {item.text}
      </ChatMarkdown>
    </div>
  );
});
