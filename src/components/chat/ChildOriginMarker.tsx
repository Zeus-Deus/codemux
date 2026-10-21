import { Bot } from "lucide-react";
import { useContext } from "react";
import { InlineButton } from "@/components/ui/button";
import { DurableTasksContext } from "./durable-task-context";
import { openDelegatedChat } from "./delegation-actions";

export function ChildOriginMarker() {
  const { origin } = useContext(DurableTasksContext);
  if (!origin) return null;
  return (
    <div
      data-testid="child-origin-marker"
      className="flex items-center gap-3 text-muted-foreground/70"
    >
      <span className="h-px flex-1 bg-border/60" />
      <span className="inline-flex min-w-0 items-center gap-1.5 font-mono text-label font-medium tracking-wide">
        <Bot className="size-3 shrink-0" aria-hidden />
        <span className="truncate">
          Subagent of · {origin.parent_title || "Parent chat"}
        </span>
        {origin.parent_exists && (
          <InlineButton
            aria-label="Open parent chat"
            onClick={() => void openDelegatedChat(origin.parent_thread_id)}
          >
            Open parent chat
          </InlineButton>
        )}
      </span>
      <span className="h-px flex-1 bg-border/60" />
    </div>
  );
}
