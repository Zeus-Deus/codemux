import { startTransition, useEffect, useState } from "react";
import { Check, Cloud } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { cn } from "@/lib/utils";
import {
  ContextMenu,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { ProjectAvatar } from "@/components/ui/project-avatar";
import { WorkingIndicator } from "@/components/ui/working-indicator";
import { IssueDetailPopover } from "@/components/github/issue-detail-popover";
import {
  PR_CHIP_TONE,
  normalizePrState,
} from "@/components/github/pr-status-icon";
import {
  WorkspaceContextMenuItems,
  DeleteWorktreeDialog,
} from "./sidebar-workspace-row";
import {
  ConfirmPushDialog,
} from "@/components/overlays/confirm-push-dialog";
import {
  activateWorkspace,
  archiveWorkspace,
  closeWorkspace,
  closeWorkspaceWithWorktree,
  unarchiveWorkspace,
  workspacePullBack,
  workspacePushToHost,
  type HostView,
} from "@/tauri/commands";
import { toast } from "@/lib/toast";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import {
  useSettingsStore,
  selectWorkingIndicator,
  selectWorkingIndicatorColor,
} from "@/stores/settings-store";
import {
  useSidebarDensityStore,
  formatElapsed,
  permissionBlockerText,
} from "@/stores/sidebar-density-store";
import { useProjectAppearance } from "./use-project-appearance";
import type { ActivePaneStatus, WorkspaceSnapshot } from "@/tauri/types";

export interface InboxRepo {
  name: string;
  path: string;
}

interface Props {
  workspace: WorkspaceSnapshot;
  repo: InboxRepo;
  isActive: boolean;
  status: ActivePaneStatus | null;
  /** Settings → Appearance → Sidebar → Show git stats. Off hides the ↑ahead
   *  and +/− numbers; the branch name stays. */
  showGitStats: boolean;
  /** Coarse (~30s) clock from the parent — one interval for the whole list. */
  now: number;
  /** True while the settle animation is collapsing this card (~200ms before
   *  it re-renders as a settled row). */
  leaving: boolean;
  /** True briefly after an un-settle so the returning card eases back in. */
  justUnsettled: boolean;
  onSettle: (workspaceId: string) => void;
}

/** One active-workspace card in the flat sidebar inbox: repo eyebrow, work
 *  title + issue chip, optional blocker line (needs-you only), and a mono
 *  meta line (branch · ↑ahead · +/− · PR chip · remote / notifications).
 *  The right side of the eyebrow shows the agent state, swapping to a
 *  "✓ Settle" button on hover/focus. */
export function SidebarInboxCard({
  workspace,
  repo,
  isActive,
  status,
  showGitStats,
  now,
  leaving,
  justUnsettled,
  onSettle,
}: Props) {
  const [showDeleteDialog, setShowDeleteDialog] = useState(false);
  const [pendingPushHost, setPendingPushHost] = useState<HostView | null>(null);

  const indicatorVariant = useSettingsStore(selectWorkingIndicator);
  const indicatorColor = useSettingsStore(selectWorkingIndicatorColor);
  const appearance = useProjectAppearance(repo.path);

  // Observe status transitions so elapsed labels (idle "26m") can be derived
  // client-side — the backend stamps no status-changed-at.
  const observeStatus = useSidebarDensityStore((s) => s.observeStatus);
  useEffect(() => {
    observeStatus(workspace.workspace_id, status);
  }, [observeStatus, workspace.workspace_id, status]);
  const settledAt = useSidebarDensityStore(
    (s) => s.settledAt[workspace.workspace_id],
  );

  const handleActivate = () => {
    useChatDraftStore.getState().setActiveDraft(null);
    startTransition(() => {
      activateWorkspace(workspace.workspace_id).catch(console.error);
    });
  };

  const isAttachOrRemote =
    workspace.attach_only === true || workspace.host_id != null;
  const isRemote = workspace.host_id !== null && workspace.host_id !== undefined;
  const isPrimary = !workspace.worktree_path;
  const canDelete =
    !isPrimary && workspace.protected !== true && !isAttachOrRemote;

  // Same non-destructive removal semantics as the workspace context menu
  // everywhere else: local rows archive (restorable, undoable), attach/remote
  // rows close (the backend refuses to archive them).
  const handleArchiveOrClose = async () => {
    try {
      if (isAttachOrRemote) {
        if (workspace.worktree_path) {
          await closeWorkspaceWithWorktree(
            workspace.workspace_id,
            false,
            false,
            false,
          );
        } else {
          await closeWorkspace(workspace.workspace_id, false);
        }
        toast.success(
          `Closed "${workspace.title}" — it stays available on its host in the Workspaces Overview`,
        );
      } else {
        const archiveId = await archiveWorkspace(workspace.workspace_id);
        toast.undoable({
          message: `Archived "${workspace.title}"`,
          description: "Restore anytime from Settings → Archive.",
          onUndo: async () => {
            await unarchiveWorkspace(archiveId);
          },
        });
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      toast.error(
        isAttachOrRemote ? "Close failed" : "Archive failed",
        { description: message },
      );
    }
  };

  const performPushToHost = async (host: HostView) => {
    try {
      const result = await workspacePushToHost(workspace.workspace_id, host.id);
      if (result.ok) {
        toast.undoable({
          message: `Pushed to ${host.name}`,
          description: "Tap Undo within 10s to pull it back.",
          onUndo: async () => {
            const undoResult = await workspacePullBack(workspace.workspace_id);
            if (undoResult.ok) {
              toast.success(`Pulled back from ${host.name}`);
            } else {
              toast.error("Pull back failed", {
                description: undoResult.message,
              });
            }
          },
        });
      } else {
        toast.error(`Push to ${host.name} failed`, {
          description: result.message,
        });
      }
    } catch (err) {
      toast.error("Push failed", {
        description: err instanceof Error ? err.message : String(err),
      });
    }
  };

  const isWorking = status === "working";
  const isNeeds = status === "permission";
  const isDone = status === "review";

  // While an agent is live and a linked issue exists, the card title IS the
  // work (issue title); the worktree name stays reachable via the branch on
  // the meta line + the title tooltip. Idle cards keep the workspace name.
  const displayTitle =
    status !== null && workspace.linked_issue
      ? workspace.linked_issue.title
      : workspace.title;
  const titleTooltip =
    status !== null && workspace.linked_issue && workspace.git_branch
      ? workspace.git_branch
      : undefined;

  const prState = normalizePrState(workspace.pr_state);
  const prMerged = prState === "merged";
  const hasAhead = showGitStats && workspace.git_ahead > 0;
  const hasStats =
    showGitStats &&
    (workspace.git_additions > 0 || workspace.git_deletions > 0);
  const idleTime =
    settledAt != null ? formatElapsed(now - settledAt) : null;

  const handlePrClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    if (workspace.pr_url) openUrl(workspace.pr_url).catch(console.error);
  };

  const stateCluster = (
    <span
      className={cn(
        "flex shrink-0 items-center gap-1.5 text-[11px]",
        // The hover/focus swap: state hides, Settle shows. CSS-only so no
        // re-render churn on pointer moves.
        "group-hover/card:hidden group-focus-within/card:hidden",
        isWorking && "font-semibold text-status-working",
        isNeeds && "font-semibold text-status-attention",
        isDone && "font-semibold text-status-open",
        !status && "font-medium text-muted-foreground/70",
      )}
    >
      {isWorking && (
        <WorkingIndicator variant={indicatorVariant} color={indicatorColor} />
      )}
      {isNeeds && (
        <span className="size-1.5 animate-pulse rounded-full bg-status-attention" />
      )}
      {isDone && <Check className="h-3 w-3" strokeWidth={2.5} />}
      {isWorking
        ? "Working"
        : isNeeds
          ? "Needs you"
          : isDone
            ? "Done · review"
            : idleTime}
    </span>
  );

  return (
    <>
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <div
            className={cn(
              "overflow-hidden transition-[max-height,opacity] duration-200 ease-out",
              leaving ? "max-h-0 opacity-0" : "max-h-40 opacity-100",
              justUnsettled && "rise-in",
            )}
          >
            <div
              role="button"
              tabIndex={0}
              data-inbox-card={workspace.workspace_id}
              onClick={handleActivate}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") handleActivate();
              }}
              className={cn(
                "group/card mb-1.5 cursor-pointer rounded-[10px] border px-[11px] pt-[9px] pb-[10px]",
                "outline-none transition-colors duration-150",
                isActive
                  ? "border-accent-ember/45 bg-foreground/[0.08]"
                  : isNeeds
                    ? "border-status-attention/30 bg-foreground/[0.03] hover:bg-foreground/[0.055]"
                    : "border-border/60 bg-foreground/[0.03] hover:bg-foreground/[0.055] focus-visible:border-border",
              )}
            >
              {/* Eyebrow: repo identity + agent state / Settle swap */}
              <div className="flex min-h-[18px] items-center gap-1.5">
                <ProjectAvatar
                  name={repo.name}
                  color={appearance.customColor}
                  imageUrl={appearance.imageUrl}
                  cacheBust={appearance.imageVersion}
                  size="sm"
                  shape="square"
                  className="font-bold"
                />
                <span className="truncate text-[11px] font-semibold tracking-[0.01em] text-muted-foreground/80">
                  {repo.name}
                </span>
                <span className="flex-1" />
                {stateCluster}
                <button
                  type="button"
                  onClick={(e) => {
                    e.stopPropagation();
                    onSettle(workspace.workspace_id);
                  }}
                  aria-label={`Settle "${workspace.title}"`}
                  className={cn(
                    "hidden h-5 shrink-0 items-center gap-1 rounded-md border border-border bg-muted px-2",
                    "text-[10.5px] font-semibold text-muted-foreground transition-colors duration-150",
                    "hover:border-muted-foreground/50 hover:text-foreground",
                    "group-hover/card:inline-flex group-focus-within/card:inline-flex",
                  )}
                >
                  <Check className="h-2.5 w-2.5" strokeWidth={2.5} />
                  Settle
                </button>
              </div>

              {/* Title line: work title + linked-issue chip */}
              <div className="mt-1 flex min-w-0 items-center gap-1.5">
                <span
                  title={titleTooltip}
                  className="truncate text-[13px] font-semibold leading-[1.35] text-foreground"
                >
                  {displayTitle}
                </span>
                {workspace.linked_issue && (
                  <IssueDetailPopover
                    workspaceId={workspace.workspace_id}
                    issue={workspace.linked_issue}
                  />
                )}
              </div>

              {/* Blocker line — needs-you cards only */}
              {isNeeds && (
                <div className="mt-0.5 truncate text-[11px] text-status-attention">
                  {permissionBlockerText(workspace)}
                </div>
              )}

              {/* Mono meta line: branch · ↑ahead · +/− · PR chip · remote/notifs */}
              <div className="mt-[5px] flex min-w-0 items-center gap-2 font-mono text-[10.5px] leading-tight text-muted-foreground/60">
                {workspace.git_branch && (
                  <span className="min-w-0 truncate">{workspace.git_branch}</span>
                )}
                {hasAhead && (
                  <span className="shrink-0 tabular-nums">
                    ↑{workspace.git_ahead}
                  </span>
                )}
                {hasStats && (
                  <span className="shrink-0 tabular-nums">
                    {workspace.git_additions > 0 && (
                      <span className="text-status-open/80">
                        +{workspace.git_additions}
                      </span>
                    )}{" "}
                    {workspace.git_deletions > 0 && (
                      <span className="text-status-attention/80">
                        −{workspace.git_deletions}
                      </span>
                    )}
                  </span>
                )}
                {prState && (
                  <button
                    type="button"
                    onClick={handlePrClick}
                    disabled={!workspace.pr_url}
                    aria-label={
                      workspace.pr_number
                        ? `Pull request #${workspace.pr_number} — ${prState}`
                        : `Pull request — ${prState}`
                    }
                    className={cn(
                      "shrink-0 rounded-[5px] border px-[5px] py-px font-mono text-[9.5px] font-medium",
                      "transition-colors duration-150",
                      PR_CHIP_TONE[prState],
                      !workspace.pr_url && "pointer-events-none",
                    )}
                  >
                    {prMerged ? "merged" : `PR #${workspace.pr_number ?? ""}`}
                  </button>
                )}
                <span className="flex-1" />
                {isRemote && (
                  <Cloud
                    aria-label="Runs on a remote host"
                    className="h-[13px] w-[13px] shrink-0 text-status-remote"
                  />
                )}
                {workspace.notification_count > 0 && (
                  <span className="flex h-[15px] min-w-[15px] shrink-0 items-center justify-center rounded-full bg-foreground/10 px-1 text-[9.5px] font-bold text-muted-foreground">
                    {workspace.notification_count}
                  </span>
                )}
              </div>
            </div>
          </div>
        </ContextMenuTrigger>
        <WorkspaceContextMenuItems
          workspace={workspace}
          onArchiveRequest={() => void handleArchiveOrClose()}
          onDeleteRequest={() => setShowDeleteDialog(true)}
          onRequestPushConfirm={(host) => setPendingPushHost(host)}
        />
      </ContextMenu>
      {canDelete && (
        <DeleteWorktreeDialog
          workspace={workspace}
          open={showDeleteDialog}
          onOpenChange={setShowDeleteDialog}
        />
      )}
      <ConfirmPushDialog
        open={pendingPushHost !== null}
        workspaceTitle={workspace.title}
        host={pendingPushHost}
        onConfirm={() => {
          if (pendingPushHost) void performPushToHost(pendingPushHost);
        }}
        onOpenChange={(open) => {
          if (!open) setPendingPushHost(null);
        }}
      />
    </>
  );
}
