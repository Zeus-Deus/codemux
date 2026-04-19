import { cn } from "@/lib/utils";
import type { PaneNodeSnapshot } from "@/tauri/types";

/**
 * Stub renderer for the `agent_chat` pane kind.
 *
 * Intentionally minimal: only displays a placeholder while the real
 * chat UI (composer, reading column, status lines, content blocks,
 * inline approvals) lands in follow-up work. The chat-ui skill
 * requires a transparent-container, pane-header-less shell the next
 * step can replace cleanly — this stub satisfies that by wrapping a
 * single centered line in a flex box.
 *
 * Uses only shadcn tokens (`bg-background`, `text-muted-foreground`)
 * so the visual target matches the rest of the app shell without
 * introducing accent color inside the pane.
 */
export function AgentChatPane({
  pane: _pane,
}: {
  pane: Extract<PaneNodeSnapshot, { kind: "agent_chat" }>;
}) {
  return (
    <div
      className={cn(
        "flex h-full w-full items-center justify-center",
        "bg-background text-muted-foreground",
      )}
    >
      <div className="text-sm">Agent chat — coming soon</div>
    </div>
  );
}
