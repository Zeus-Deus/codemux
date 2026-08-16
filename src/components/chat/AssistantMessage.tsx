import { memo } from "react";

import { MessageCopyButton } from "@/components/chat/MessageCopyButton";
import type { AssistantMessageItem } from "@/lib/agent-chat/types";

import { ChatMarkdown } from "./ChatMarkdown";

/**
 * Assistant prose (design D4). No bubble, no avatar (the avatar lives in
 * the turn gutter, drawn once by MessageList) — just the streaming
 * markdown at 14px / 1.625 line-height. All element styling lives in
 * `ChatMarkdown` so the plan renderer and reasoning body share one prose
 * treatment. Live progress is communicated by the transcript's dedicated
 * streaming marker rather than extra chrome appended to the prose.
 *
 * A settled message gets a footer strip under the prose that fades in on hover
 * or keyboard focus, holding a copy action aligned to the start of the text.
 * It copies `item.text` — the markdown source, not the rendered DOM — so
 * pasting into another editor keeps the formatting. It stays hidden while the
 * message streams, where only half the answer exists yet.
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
  return (
    <div className="group conversation-text leading-relaxed text-foreground break-words">
      <ChatMarkdown
        streaming={item.streaming}
        workspaceId={workspaceId}
        cwd={cwd}
      >
        {item.text}
      </ChatMarkdown>
      {!item.streaming && item.text ? (
        <MessageCopyButton
          text={item.text}
          label="Copy response"
          className="mt-1"
        />
      ) : null}
    </div>
  );
});
