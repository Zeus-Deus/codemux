import { memo } from "react";

import { MessageCopyButton } from "@/components/chat/MessageCopyButton";
import { MESSAGE_GROUP_CLASS } from "@/components/chat/message-action";
import type { AssistantMessageItem } from "@/lib/agent-chat/types";
import { cn } from "@/lib/utils";

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
 * message streams, where only half the answer exists yet. The hover target is
 * a *named* group: prose can contain its own unnamed `group`s (an inline image
 * zooms on hover), which an unnamed one here would fire on every hover.
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
    <div
      className={cn(
        MESSAGE_GROUP_CLASS,
        "conversation-text leading-relaxed text-foreground break-words",
      )}
    >
      <ChatMarkdown
        streaming={item.streaming}
        workspaceId={workspaceId}
        cwd={referenceCwd ?? cwd}
        workspaceCwd={cwd}
        referencePaths={referencePaths}
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
