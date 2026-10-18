import { Eye } from "lucide-react";

import { Spinner } from "@/components/ui/spinner";
import { useActiveChatTasks } from "@/hooks/use-active-chat-tasks";
import { usePrWatch } from "@/hooks/use-pr-watch";
import {
  PR_WATCH_TONE_CLASS,
  canWatchPr,
  describePrWatch,
  watchTargetThreadId,
} from "@/lib/pr-watch";
import { cn } from "@/lib/utils";
import type { WorkspaceSnapshot } from "@/tauri/types";

import { DETAILS_ICON, DETAILS_LABEL, DETAILS_ROW, DETAILS_TRAILING } from "./styles";

/**
 * "Watch CI & reviews" under the pull request row: the workspace's chat is
 * woken when a check fails, someone else comments or reviews, or the branch
 * conflicts. Acts for the focused chat, else the workspace's first chat.
 */
export function PrWatchRow({ workspace }: { workspace: WorkspaceSnapshot }) {
  const { threadId: focused } = useActiveChatTasks(workspace);
  const threadId = canWatchPr(workspace) ? watchTargetThreadId(workspace, focused) : null;
  const { watch, load, busy, toggle } = usePrWatch(threadId);
  if (!canWatchPr(workspace)) return null;

  const view = describePrWatch(watch);
  const loaded = load === "loaded";
  const title = !threadId
    ? "Open an agent chat in this workspace to watch its pull request"
    : load === "failed"
      ? "Couldn't read the watch status. Click to try again."
      : loaded
        ? view.title
        : "Reading the watch status…";
  return (
    <button
      type="button"
      data-testid="workspace-details-pr-watch"
      disabled={!threadId || load === "loading" || busy}
      onClick={() => toggle(workspace.pr_number)}
      className={DETAILS_ROW}
      title={title}
      aria-pressed={loaded ? view.active : undefined}
    >
      <Eye className={DETAILS_ICON} />
      <span className={DETAILS_LABEL}>Watch CI &amp; reviews</span>
      <span
        className={cn(
          DETAILS_TRAILING,
          "font-sans",
          load === "failed" ? "text-danger" : PR_WATCH_TONE_CLASS[view.tone],
        )}
      >
        {busy ? (
          <Spinner className="size-3" />
        ) : load === "failed" ? (
          "Retry"
        ) : loaded ? (
          view.label
        ) : (
          "…"
        )}
      </span>
    </button>
  );
}
