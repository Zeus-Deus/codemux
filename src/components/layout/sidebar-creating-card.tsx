import { memo } from "react";
import { CircleDotDashed } from "lucide-react";

import { ProjectAvatar } from "@/components/ui/project-avatar";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import type { ChatDraft } from "@/stores/chat-draft-store";
import { useProjectAppearance } from "./use-project-appearance";

/** The ambient progress hairline along a card's bottom edge while a
 *  workspace is being created or its agent is starting. Shared with the real
 *  card so the hand-off keeps the same motion running. */
export function CardProgressSweep() {
  return (
    <span
      aria-hidden="true"
      className="pointer-events-none absolute inset-x-2.5 bottom-0 h-px overflow-hidden"
    >
      <span className="cm-sweep absolute left-0 top-0 h-px w-[38%] bg-gradient-to-r from-transparent via-accent-ember to-transparent" />
    </span>
  );
}

/** The project's own colour / image, so the stand-in already wears the
 *  avatar the real card will. */
function ThemedProjectAvatar({ name, path }: { name: string; path: string }) {
  const appearance = useProjectAppearance(path);
  return (
    <ProjectAvatar
      name={name}
      color={appearance.customColor}
      imageUrl={appearance.imageUrl}
      cacheBust={appearance.imageVersion}
      size="sm"
      shape="square"
      className="shrink-0 font-bold"
    />
  );
}

/**
 * Stand-in for a workspace the first prompt is creating right now.
 *
 * Sending from a new chat retires the sidebar's draft row on the spot, but
 * the workspace it becomes only reaches the snapshot seconds later (git
 * worktree, pane, session). This card fills that gap in the exact slot the
 * real card will take — newest-first, top of the active list — and copies
 * its box metrics, so the hand-off is a cross-fade rather than a jump.
 */
export const SidebarCreatingCard = memo(function SidebarCreatingCard({
  draft,
  projectName,
  projectPath,
}: {
  draft: ChatDraft;
  projectName: string;
  projectPath: string | null;
}) {
  // The prompt that was just sent — the workspace is named from it, so it is
  // the most honest title available before the backend names the real one.
  const prompt = useAgentChatStore((s) => {
    const first = s.threads[draft.threadId]?.messages.find(
      (m) => m.kind === "user_message",
    );
    return first && first.kind === "user_message" ? first.text : null;
  });
  const title = prompt?.trim().split("\n", 1)[0] || "New workspace";
  const step =
    draft.checkoutMode === "worktree"
      ? "Creating worktree…"
      : "Setting up workspace…";

  return (
    <div
      data-creating-card={draft.draftId}
      className="card-in overflow-hidden"
    >
      <div
        role="status"
        aria-label={`Creating workspace: ${title}`}
        className="relative mb-1.5 select-none rounded-lg border border-border bg-surface-3 px-2.5 py-2"
      >
        <div className="flex min-h-5 items-center gap-1.5">
          {projectPath ? (
            <ThemedProjectAvatar name={projectName} path={projectPath} />
          ) : (
            <ProjectAvatar
              name={projectName}
              size="sm"
              shape="square"
              className="shrink-0 font-bold"
            />
          )}
          <span className="min-w-0 truncate text-label font-medium tracking-[0.01em] text-muted-foreground/80">
            {projectName}
          </span>
          <span className="ml-auto flex shrink-0 items-center gap-1.5 text-label font-semibold text-muted-foreground">
            <CircleDotDashed
              aria-hidden
              className="size-3.5 motion-safe:animate-spin [animation-duration:2.4s]"
            />
            <span className="shimmer">Creating</span>
          </span>
        </div>
        <div className="mt-0.5 truncate text-body font-medium leading-[1.35] text-foreground">
          {title}
        </div>
        <div className="mt-1 truncate font-mono text-label leading-tight text-muted-foreground/60">
          {step}
        </div>
        <CardProgressSweep />
      </div>
    </div>
  );
});
