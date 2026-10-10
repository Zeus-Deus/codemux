import { FileDiff, GitPullRequest } from "lucide-react";
import { useShallow } from "zustand/react/shallow";
import { useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";

/** The phone hides the desktop's status row under the composer, so the two
 *  facts worth a glance mid-conversation — what changed, and the PR — ride
 *  just above it as one-tap shortcuts into the matching tool. */
export function MobileComposerChips({ workspaceId }: { workspaceId: string }) {
  const summary = useAppStore(
    useShallow((s) => {
      const ws = s.appState?.workspaces.find(
        (w) => w.workspace_id === workspaceId,
      );
      return {
        files: ws?.git_changed_files ?? 0,
        additions: ws?.git_additions ?? 0,
        deletions: ws?.git_deletions ?? 0,
        pr: ws?.pr_number ?? null,
      };
    }),
  );
  const open = (tab: "changes" | "review") =>
    useUIStore.getState().setRightPanelTab(workspaceId, tab);
  if (summary.files === 0 && summary.pr === null) return null;
  return (
    <div className="mobile-composer-chips" data-testid="mobile-composer-chips">
      {summary.files > 0 && (
        <button
          type="button"
          aria-label={`Review ${summary.files} changed ${summary.files === 1 ? "file" : "files"}`}
          onClick={() => open("changes")}
        >
          <FileDiff className="size-3.5" aria-hidden />
          {summary.files} {summary.files === 1 ? "file" : "files"}
          {summary.additions > 0 && (
            <span className="text-status-open">+{summary.additions}</span>
          )}
          {summary.deletions > 0 && (
            <span className="text-status-attention">−{summary.deletions}</span>
          )}
        </button>
      )}
      {summary.pr !== null && (
        <button
          type="button"
          aria-label={`Open pull request #${summary.pr}`}
          onClick={() => open("review")}
        >
          <GitPullRequest className="size-3.5" aria-hidden />#{summary.pr}
        </button>
      )}
    </div>
  );
}
