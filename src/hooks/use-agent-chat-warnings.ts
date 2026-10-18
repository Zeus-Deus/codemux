import { useCallback } from "react";

import { toast } from "@/lib/toast";
import { onAgentChatGlobalEvent, type AgentChatEventPayload } from "@/tauri/events";
import { useTauriEvent } from "./use-tauri-event";

/** Durability warnings must reach the user even when no chat pane is attached. */
export function useAgentChatWarnings(): void {
  const handleWarning = useCallback(({ event }: AgentChatEventPayload) => {
    if (event.type !== "runtime_warning") return;
    const details = event.original_payload;
    if (!details || typeof details !== "object" || !("code" in details)) return;
    const title =
      details.code === "provider_event_lag"
        ? "Provider events were lost"
        : details.code === "transcript_write_failed"
          ? "Couldn't save conversation history"
          : null;
    if (!title) return;
    console.error("[agent-chat]", event.message, details);
    // A burst of failed writes updates one toast instead of filling the screen.
    toast.error(title, {
      id: `agent-chat-${details.code}`,
      description: event.message,
    });
  }, []);

  useTauriEvent(onAgentChatGlobalEvent, handleWarning, [handleWarning]);
}
