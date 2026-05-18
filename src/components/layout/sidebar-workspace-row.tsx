import { startTransition, useEffect, useRef, useState } from "react";
import {
  ContextMenu,
  ContextMenuTrigger,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubTrigger,
  ContextMenuSubContent,
} from "@/components/ui/context-menu";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import {
  X,
  Laptop,
  GitBranch,
  Workflow,
  AlertTriangle,
  BellOff,
  Cloud,
  Loader2,
} from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { PrStatusIcon, humanizePrState, prStatusToneClass } from "@/components/github/pr-status-icon";
import {
  activateWorkspace,
  checkoutDefaultBranchInWorkspace,
  closeWorkspace,
  closeWorkspaceWithWorktree,
  renameWorkspace,
  setWorkspaceMuted,
  detectEditors,
  openInEditor,
  runWorkspaceSetup,
  workspacePullBack,
  workspacePushToHost,
  type HostView,
} from "@/tauri/commands";
import { useHosts } from "@/stores/hosts-store";
import type { WorkspaceSnapshot, EditorInfo, ActivePaneStatus } from "@/tauri/types";
import { useAppStore } from "@/stores/app-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { getWorkspaceStatus } from "@/lib/pane-status";
import { StatusIndicator } from "@/components/ui/status-indicator";
import { IssueDetailPopover } from "@/components/github/issue-detail-popover";
import { toast } from "@/lib/toast";
import { useDefaultBranch } from "./default-branch-cache";

interface Props {
  workspace: WorkspaceSnapshot;
  isActive: boolean;
}

function RemoveWorkspaceDialog({
  workspace,
  open,
  onOpenChange,
}: {
  workspace: WorkspaceSnapshot;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const isWorktree = !!workspace.worktree_path;
  const [deleteBranch, setDeleteBranch] = useState(true);

  const hasUnpushed = workspace.git_ahead > 0;
  const hasUncommitted = workspace.git_changed_files > 0;
  const hasWarnings = hasUnpushed || hasUncommitted;

  const warningMessage = hasUnpushed && hasUncommitted
    ? "Has uncommitted changes and unpushed commits"
    : hasUncommitted
      ? "Has uncommitted changes"
      : hasUnpushed
        ? "Has unpushed commits"
        : null;

  const handleHide = () => {
    if (isWorktree) {
      closeWorkspaceWithWorktree(workspace.workspace_id, false, false, false).catch(console.error);
    } else {
      closeWorkspace(workspace.workspace_id, false).catch(console.error);
    }
    onOpenChange(false);
  };

  const handleDelete = () => {
    if (isWorktree) {
      closeWorkspaceWithWorktree(workspace.workspace_id, true, deleteBranch, false).catch(console.error);
    } else {
      closeWorkspace(workspace.workspace_id, true).catch(console.error);
    }
    onOpenChange(false);
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent showCloseButton={false} className="max-w-[340px]">
        <DialogHeader>
          <DialogTitle className="text-sm">
            {isWorktree
              ? <>Remove workspace &ldquo;{workspace.title}&rdquo;?</>
              : <>Close workspace &ldquo;{workspace.title}&rdquo;?</>}
          </DialogTitle>
          <DialogDescription>
            {isWorktree
              ? "Deleting will permanently remove the worktree. You can hide instead to keep files on disk."
              : "Removes this workspace from the sidebar. Project files on disk are untouched."}
          </DialogDescription>
        </DialogHeader>

        {hasWarnings && (
          <div className="flex items-center gap-2 rounded-md border border-yellow-500/20 bg-yellow-500/10 px-2.5 py-1.5 text-xs text-yellow-400">
            <AlertTriangle className="h-3.5 w-3.5 shrink-0" />
            {warningMessage}
          </div>
        )}

        {isWorktree && (
          <label className="flex items-center gap-2 cursor-pointer select-none">
            <input
              type="checkbox"
              checked={deleteBranch}
              onChange={(e) => setDeleteBranch(e.target.checked)}
              className="rounded border-border"
            />
            <span className="text-xs text-muted-foreground">
              Also delete local branch
            </span>
          </label>
        )}

        <div className="flex justify-end gap-2 pt-1">
          <Button
            variant="ghost"
            size="sm"
            className="h-7 px-3 text-xs"
            onClick={() => onOpenChange(false)}
          >
            Cancel
          </Button>
          {isWorktree ? (
            <>
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="secondary"
                    size="sm"
                    className="h-7 px-3 text-xs"
                    onClick={handleHide}
                  >
                    Hide
                  </Button>
                </TooltipTrigger>
                <TooltipContent side="bottom" sideOffset={4} className="text-xs">
                  Remove from sidebar. Worktree files stay on disk.
                </TooltipContent>
              </Tooltip>
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="destructive"
                    size="sm"
                    className="h-7 px-3 text-xs"
                    onClick={handleDelete}
                  >
                    Delete
                  </Button>
                </TooltipTrigger>
                <TooltipContent side="bottom" sideOffset={4} className="text-xs">
                  Permanently remove worktree directory from disk.
                </TooltipContent>
              </Tooltip>
            </>
          ) : (
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="secondary"
                  size="sm"
                  className="h-7 px-3 text-xs"
                  onClick={handleHide}
                >
                  Close
                </Button>
              </TooltipTrigger>
              <TooltipContent side="bottom" sideOffset={4} className="text-xs">
                Remove from sidebar. Project files on disk are untouched.
              </TooltipContent>
            </Tooltip>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}

export function WorkspaceContextMenuItems({
  workspace,
  onRemoveRequest,
}: {
  workspace: WorkspaceSnapshot;
  onRemoveRequest: () => void;
}) {
  const [editors, setEditors] = useState<EditorInfo[]>([]);
  const isWorktree = !!workspace.worktree_path;
  const defaultBranch = useDefaultBranch(
    workspace.project_root ?? (isWorktree ? null : workspace.cwd),
  );

  const hosts = useHosts();

  useEffect(() => {
    detectEditors().then(setEditors).catch(console.error);
  }, []);

  const isRemote =
    workspace.host_id !== null && workspace.host_id !== undefined;
  const setPushPullInFlight = useAppStore(
    (s) => s.setWorkspacePushPullInFlight,
  );

  const handleMoveToHost = async (host: HostView) => {
    setPushPullInFlight(workspace.workspace_id);
    try {
      const result = await workspacePushToHost(
        workspace.workspace_id,
        host.id,
      );
      if (result.ok) {
        toast.success(`Pushed to ${host.name}`, {
          description: result.message,
        });
      } else {
        toast.error(`Push to ${host.name} failed`, {
          description: result.message,
        });
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      toast.error("Push failed", { description: message });
    } finally {
      setPushPullInFlight(null);
    }
  };

  const handlePullBack = async () => {
    setPushPullInFlight(workspace.workspace_id);
    try {
      const result = await workspacePullBack(workspace.workspace_id);
      if (result.ok) {
        toast.success("Pulled back to this device", {
          description: result.message,
        });
      } else {
        toast.error("Pull back failed", {
          description: result.message,
        });
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      toast.error("Pull back failed", { description: message });
    } finally {
      setPushPullInFlight(null);
    }
  };

  const handleRename = () => {
    const newTitle = window.prompt("Rename workspace", workspace.title);
    if (newTitle && newTitle !== workspace.title) {
      renameWorkspace(workspace.workspace_id, newTitle).catch(console.error);
    }
  };

  const handleCopyBranch = () => {
    if (workspace.git_branch) {
      navigator.clipboard.writeText(workspace.git_branch).catch(console.error);
    }
  };

  const handleToggleMute = () => {
    setWorkspaceMuted(
      workspace.workspace_id,
      !workspace.notifications_muted,
    ).catch(console.error);
  };

  const handleOpenInEditor = (editorId: string) => {
    openInEditor(editorId, workspace.cwd).catch(console.error);
  };

  const handleCheckoutDefault = async () => {
    try {
      const branch = await checkoutDefaultBranchInWorkspace(workspace.workspace_id);
      toast.success(`Switched to ${branch}`);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      toast.error(`Couldn't switch: ${message}`);
    }
  };

  const showCheckoutDefault = !isWorktree;
  const isOnDefault =
    defaultBranch !== null && workspace.git_branch === defaultBranch;

  return (
    <ContextMenuContent>
      <ContextMenuItem onClick={handleRename}>
        Rename workspace
      </ContextMenuItem>
      {editors.length === 1 ? (
        <ContextMenuItem onClick={() => handleOpenInEditor(editors[0].id)}>
          Open in {editors[0].name}
        </ContextMenuItem>
      ) : editors.length > 1 ? (
        <ContextMenuSub>
          <ContextMenuSubTrigger>Open in editor</ContextMenuSubTrigger>
          <ContextMenuSubContent>
            {editors.map((editor) => (
              <ContextMenuItem key={editor.id} onClick={() => handleOpenInEditor(editor.id)}>
                {editor.name}
              </ContextMenuItem>
            ))}
          </ContextMenuSubContent>
        </ContextMenuSub>
      ) : null}
      <ContextMenuItem
        onClick={handleCopyBranch}
        disabled={!workspace.git_branch}
      >
        Copy branch name
      </ContextMenuItem>
      {showCheckoutDefault && (
        <ContextMenuItem
          onClick={handleCheckoutDefault}
          disabled={isOnDefault}
        >
          Checkout default branch
        </ContextMenuItem>
      )}
      <ContextMenuItem
        onClick={() => runWorkspaceSetup(workspace.workspace_id).catch(console.error)}
      >
        Re-run Setup
      </ContextMenuItem>
      <ContextMenuItem onClick={handleToggleMute}>
        {workspace.notifications_muted
          ? "Unmute notifications"
          : "Mute notifications"}
      </ContextMenuItem>

      <ContextMenuSeparator />
      {isRemote ? (
        <ContextMenuItem onClick={() => void handlePullBack()}>
          Pull back to this device
        </ContextMenuItem>
      ) : hosts.length > 0 ? (
        <ContextMenuSub>
          <ContextMenuSubTrigger>Move to host…</ContextMenuSubTrigger>
          <ContextMenuSubContent>
            {hosts.map((host) => (
              <ContextMenuItem
                key={host.id}
                onClick={() => void handleMoveToHost(host)}
              >
                {host.name}
              </ContextMenuItem>
            ))}
          </ContextMenuSubContent>
        </ContextMenuSub>
      ) : (
        <ContextMenuItem
          disabled
          title="Add hosts in Settings → Hosts to push workspaces"
        >
          Move to host… (no hosts configured)
        </ContextMenuItem>
      )}

      <ContextMenuSeparator />
      <ContextMenuItem onClick={onRemoveRequest}>
        Close Worktree
      </ContextMenuItem>
    </ContextMenuContent>
  );
}

const SPINNER_FRAMES = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

function AsciiSpinner() {
  const [frame, setFrame] = useState(0);
  const frameRef = useRef(0);

  useEffect(() => {
    const id = setInterval(() => {
      frameRef.current = (frameRef.current + 1) % SPINNER_FRAMES.length;
      setFrame(frameRef.current);
    }, 80);
    return () => clearInterval(id);
  }, []);

  return (
    <span className="text-amber-500 text-sm leading-none select-none" aria-label="Agent working">
      {SPINNER_FRAMES[frame]}
    </span>
  );
}

export function SidebarWorkspaceRow({ workspace, isActive }: Props) {
  const [showRemoveDialog, setShowRemoveDialog] = useState(false);

  const workspaceStatus: ActivePaneStatus | null = useAppStore((s) => {
    if (!s.appState) return null;
    return getWorkspaceStatus(workspace.surfaces, s.appState.pane_statuses);
  });

  const handleActivate = () => {
    useChatDraftStore.getState().setActiveDraft(null);
    startTransition(() => {
      activateWorkspace(workspace.workspace_id).catch(console.error);
    });
  };

  const isPrimary = !workspace.worktree_path;
  const canDelete = !isPrimary;
  const isRemote =
    workspace.host_id !== null && workspace.host_id !== undefined;
  const isPushOrPullInFlight = useAppStore(
    (s) => s.workspacePushPullInFlight === workspace.workspace_id,
  );
  const icon = isPushOrPullInFlight ? (
    <Loader2 className="h-3.5 w-3.5 shrink-0 text-muted-foreground animate-spin" />
  ) : isRemote ? (
    <Cloud className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
  ) : workspace.workspace_type === "open_flow" ? (
    <Workflow className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
  ) : isPrimary ? (
    <Laptop className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
  ) : (
    <GitBranch className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
  );

  const showPrIcon = !!workspace.pr_state && workspaceStatus !== "working";
  const prHumanState = humanizePrState(workspace.pr_state);
  const prToneCls = prStatusToneClass(workspace.pr_state);
  const handlePrClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    if (workspace.pr_url) {
      openUrl(workspace.pr_url).catch(console.error);
    }
  };

  const hasDiff = workspace.git_additions > 0 || workspace.git_deletions > 0;
  const hasAheadBehind = workspace.git_ahead > 0 || workspace.git_behind > 0;

  return (
    <>
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <div
            role="button"
            tabIndex={0}
            onClick={handleActivate}
            onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") handleActivate(); }}
            className={cn(
              "group/row mx-1.5 flex pl-[18px] pr-2 py-1 text-sm cursor-pointer relative rounded-lg",
              "hover:bg-muted/40 transition-colors",
              isActive && "bg-muted",
            )}
          >
            {/* Icon column — size-5 to subordinate to project avatar */}
            <div className="relative size-5 flex items-center justify-center shrink-0 mr-2">
              {workspaceStatus === "working" ? (
                <AsciiSpinner />
              ) : (
                <>
                  {icon}
                  {workspaceStatus && (
                    <StatusIndicator
                      status={workspaceStatus}
                      className="absolute -top-0.5 -right-0.5"
                    />
                  )}
                </>
              )}
            </div>

            <div className="flex-1 min-w-0">
              {/* Line 1: title + notification badge (kept on top
                  because notifications are a needs-attention signal
                  the user should see at a glance). */}
              <div className="flex items-center gap-1.5">
                <span className={cn(
                  "truncate text-[13px] leading-tight",
                  isActive ? "text-foreground font-medium" : "text-foreground/85",
                )}>
                  {workspace.title}
                </span>

                {workspace.notification_count > 0 && (
                  <Badge
                    variant="outline"
                    className={cn(
                      "shrink-0 ml-auto text-[10px] tabular-nums text-warning bg-warning/15 border-transparent px-1.5 py-0 leading-[14px] h-[14px]",
                      canDelete && "transition-opacity group-hover/row:opacity-0",
                    )}
                  >
                    {workspace.notification_count}
                  </Badge>
                )}
              </div>

              {/* Line 2: branch + ahead/behind + diff stats +
                  indicators. Everything that's optional only renders
                  when relevant — when none of these apply the row is
                  one line, keeping the sidebar calm. */}
              {(workspace.git_branch || hasDiff || hasAheadBehind) && (
                <div className="flex items-center gap-1.5 text-[11px] text-muted-foreground/60 font-mono leading-tight mt-0.5">
                  {workspace.git_branch && (
                    <span className="truncate min-w-0">{workspace.git_branch}</span>
                  )}

                  {hasAheadBehind && (
                    <span className="flex items-center gap-1 shrink-0 tabular-nums">
                      {workspace.git_behind > 0 && (
                        <span className="text-warning/80">↓{workspace.git_behind}</span>
                      )}
                      {workspace.git_ahead > 0 && (
                        <span className="text-success/80">↑{workspace.git_ahead}</span>
                      )}
                    </span>
                  )}

                  {/* diff stats inline (no pill background) — fades on
                      hover so the hover-reveal X has its slot. */}
                  {hasDiff && (
                    <span className={cn(
                      "flex items-center gap-1 shrink-0 tabular-nums ml-auto",
                      canDelete && "transition-opacity group-hover/row:opacity-0",
                    )}>
                      {workspace.git_additions > 0 && (
                        <span className="text-success/80">+{workspace.git_additions}</span>
                      )}
                      {workspace.git_deletions > 0 && (
                        <span className="text-danger/80">−{workspace.git_deletions}</span>
                      )}
                    </span>
                  )}

                  {/* Indicator cluster — muted bell, linked issue, PR.
                      Only renders when at least one is present. */}
                  {(workspace.linked_issue || showPrIcon || workspace.notifications_muted) && (
                    <div className={cn(
                      "flex items-center gap-1 shrink-0",
                      !hasDiff && "ml-auto",
                    )}>
                      {workspace.notifications_muted && (
                        <Tooltip>
                          <TooltipTrigger asChild>
                            <BellOff
                              className="h-3 w-3 text-muted-foreground/60"
                              aria-label="Notifications muted"
                            />
                          </TooltipTrigger>
                          <TooltipContent side="bottom" sideOffset={4} className="text-xs">
                            Agent notifications muted
                          </TooltipContent>
                        </Tooltip>
                      )}
                      {workspace.linked_issue && (
                        <IssueDetailPopover
                          workspaceId={workspace.workspace_id}
                          issue={workspace.linked_issue}
                        />
                      )}
                      {showPrIcon && (
                        <button
                          type="button"
                          onClick={handlePrClick}
                          disabled={!workspace.pr_url}
                          aria-label={
                            workspace.pr_number
                              ? `Open PR #${workspace.pr_number} on GitHub — ${prHumanState ?? "Pull request"}`
                              : `Open pull request on GitHub — ${prHumanState ?? ""}`
                          }
                          className={cn(
                            "inline-flex items-center gap-0.5 rounded-full px-1.5 py-px transition-opacity",
                            prToneCls,
                            workspace.pr_url ? "hover:opacity-80" : "cursor-not-allowed opacity-60",
                          )}
                        >
                          <PrStatusIcon state={workspace.pr_state} size={3} />
                          {workspace.pr_number && (
                            <span className="text-[10px] tabular-nums text-muted-foreground/60">
                              #{workspace.pr_number}
                            </span>
                          )}
                        </button>
                      )}
                    </div>
                  )}
                </div>
              )}
            </div>

            {/* Hover-reveal remove button — pinned to a fixed slot so
                it lands at the same x-coordinate on every row. */}
            {canDelete && (
              <Button
                variant="ghost"
                size="icon-xs"
                className="absolute right-2 top-1/2 -translate-y-1/2 opacity-0 group-hover/row:opacity-100 transition-opacity text-muted-foreground hover:text-foreground"
                onClick={(e) => { e.stopPropagation(); setShowRemoveDialog(true); }}
                aria-label="Remove workspace"
              >
                <X className="h-3.5 w-3.5" />
              </Button>
            )}
          </div>
        </ContextMenuTrigger>
        <WorkspaceContextMenuItems
          workspace={workspace}
          onRemoveRequest={() => setShowRemoveDialog(true)}
        />
      </ContextMenu>
      <RemoveWorkspaceDialog
        workspace={workspace}
        open={showRemoveDialog}
        onOpenChange={setShowRemoveDialog}
      />
    </>
  );
}
