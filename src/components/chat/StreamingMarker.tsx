import { LoaderCircle } from "lucide-react";

import type { ChatViewItem } from "@/lib/agent-chat/types";

/**
 * Tail "working" marker (design D9) — the last row inside the scroller
 * while a turn is in flight and no approval is pending. An ember spinner
 * sits in the 29px turn gutter (aligned under the assistant avatar) next
 * to a shimmering status line. Gating lives in
 * `shouldShowThinkingIndicator`; this component only derives the label.
 */
export function StreamingMarker({ messages }: { messages: ChatViewItem[] }) {
  const label = deriveStreamingLabel(messages);
  return (
    <div
      className="flex items-center gap-[13px] pt-0.5"
      role="status"
      aria-label="Agent is working"
    >
      <span className="flex w-[29px] shrink-0 justify-center">
        <LoaderCircle
          className="h-[15px] w-[15px] animate-spin text-accent-ember"
          strokeWidth={1.6}
          aria-hidden
        />
      </span>
      <span className="shimmer text-[12.5px] font-semibold">{label}</span>
    </div>
  );
}

/** Status line derived from the transcript tail. Running tool → "Running
 *  <tool>…", streaming prose → "Writing…", streaming reasoning →
 *  "Thinking…", otherwise the neutral "Working…". */
export function deriveStreamingLabel(messages: ChatViewItem[]): string {
  const last = messages[messages.length - 1];
  if (!last) return "Working…";
  switch (last.kind) {
    case "tool_call":
      return last.status === "running" ? `Running ${last.tool_name}…` : "Working…";
    case "assistant_message":
      return last.streaming ? "Writing…" : "Working…";
    case "reasoning":
      return last.streaming ? "Thinking…" : "Working…";
    default:
      return "Working…";
  }
}
