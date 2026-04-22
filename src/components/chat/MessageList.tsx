import { useMemo } from "react";

import type { ChatViewItem } from "@/lib/agent-chat/types";
import type { ApprovalDecision } from "@/tauri/events";

import { AssistantMessage } from "./AssistantMessage";
import { PermissionRequestBlock } from "./PermissionRequestBlock";
import { ToolCallBlock } from "./ToolCallBlock";
import { ToolCallStatus } from "./ToolCallStatus";
import { UserMessage } from "./UserMessage";

interface Props {
  messages: ChatViewItem[];
  onRespondToRequest: (requestId: string, decision: ApprovalDecision) => void;
}

export function MessageList({ messages, onRespondToRequest }: Props) {
  // Sort by seq so order is a property of the data, not of React
  // reconciliation or store-update timing. Stable for equal seq via
  // id tiebreak (should never happen in practice — seq is unique
  // per-thread — but cheap insurance for future callers).
  const ordered = useMemo(() => {
    const copy = messages.slice();
    copy.sort((a, b) => a.seq - b.seq || a.id.localeCompare(b.id));
    return copy;
  }, [messages]);

  return (
    <div className="flex flex-col gap-3">
      {ordered.map((item) => (
        <MessageRow
          key={item.id}
          item={item}
          onRespondToRequest={onRespondToRequest}
        />
      ))}
    </div>
  );
}

function MessageRow({
  item,
  onRespondToRequest,
}: {
  item: ChatViewItem;
  onRespondToRequest: (requestId: string, decision: ApprovalDecision) => void;
}) {
  switch (item.kind) {
    case "user_message":
      return <UserMessage item={item} />;
    case "assistant_message":
      return <AssistantMessage item={item} />;
    case "tool_call":
      return (
        <div className="space-y-1">
          <ToolCallStatus item={item} />
          {hasRenderableContent(item.result_content) && (
            <ToolCallBlock
              content={item.result_content}
              error={item.status === "error"}
            />
          )}
        </div>
      );
    case "permission_request":
      return (
        <PermissionRequestBlock
          item={item}
          onDecide={(decision) => onRespondToRequest(item.request_id, decision)}
        />
      );
    case "turn_ended":
      if (item.status.kind !== "error") return null;
      return (
        <div className="py-0.5 text-xs text-muted-foreground">
          Turn ended: {item.status.subtype}
          {item.status.message ? ` — ${item.status.message}` : ""}
        </div>
      );
  }
}

function hasRenderableContent(content: unknown): boolean {
  if (content == null) return false;
  if (typeof content === "string") return content.length > 0;
  if (Array.isArray(content)) return content.length > 0;
  return true;
}
