import type { AssistantMessageItem } from "@/lib/agent-chat/types";

import { ChatMarkdown } from "./ChatMarkdown";
import { StreamingIndicator } from "./streaming-indicator";

/**
 * Prose-only rendering. Per the chat-ui skill:
 *  - no container / border / avatar / label
 *  - markdown headers render as emphasized prose, NOT <h1>/<h2>
 *  - inline code keeps the prose font with a subtle background
 *  - fenced code blocks share the same neutral monospace styling as
 *    tool-output blocks (see ToolCallBlock)
 *  - bold/italic only when the source asks for it
 *
 * All component overrides live in `ChatMarkdown` so the plan renderer
 * (Stage 2) and future mode-pill renderers share the exact same prose
 * without drift.
 */
export function AssistantMessage({ item }: { item: AssistantMessageItem }) {
  const showIndicator = item.streaming && item.text.length === 0;

  return (
    <div className="text-sm leading-relaxed text-foreground break-words">
      <ChatMarkdown>{item.text}</ChatMarkdown>
      {item.streaming && !showIndicator && (
        <StreamingIndicator className="ml-0.5" />
      )}
      {showIndicator && <StreamingIndicator />}
    </div>
  );
}
