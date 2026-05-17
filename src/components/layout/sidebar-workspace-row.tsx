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

        {/* Warning banner */}
        {hasWarnings && (
          <div className="flex items-center gap-2 rounded-md border border-yellow-500/20 bg-yellow-500/10 px-2.5 py-1.5 text-xs text-yellow-400">
            <AlertTriangle className="h-3.5 w-3.5 shrink-0" />
            {warningMessage}
          </div>
        )}

        {/* Delete branch checkbox */}
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
  // Default-branch lookup is scoped to the project_root (the real repo root
  // shared by all workspaces pointing at the same repo). Falls back to cwd
  // for primary workspaces that haven't had project_root stamped yet.
  const defaultBranch = useDefaultBranch(
    workspace.project_root ?? (isWorktree ? null : workspace.cwd),
  );

  // Hosts feed the "Move to host..." submenu. Reads from the shared
  // store cache so N workspace rows share ONE IPC round-trip instead
  // of one each. See `src/stores/hosts-store.ts`.
  const hosts = useHosts();

  useEffect(() => {
    detectEditors().then(setEditors).catch(console.error);
  }, []);

  const isRemote =
    workspace.host_id !== null && workspace.host_id !== undefined;
  const setPushPullInFlight = useAppStore(
    (s) => s.setWorkspacePushPullInFlight,
  );

  // Push the workspace to the chosen host. The backend atomically
  // sets host_id only on successful rsync, so we don't need the
  // optimistic-set + rollback dance that used to flicker the icon.
  // On failure the workspace stays local with a toast.
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

  // Show the "Checkout default branch" action only for primary workspaces
  // (worktree workspaces live on a fixed branch by design — swapping their
  // HEAD would break the Codemux worktree model). Disable when we already
  // know we're on the default branch — gated on `defaultBranch` being
  // resolved so the action stays clickable while the fetch is pending, and
  // the backend's `Ok(None)` guard handles the "actually on default" edge.
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

      {/* Cloud-push (step 2): Move to host… / Pull back. Position is
          between mute and Close Worktree so destructive actions stay
          at the bottom. Move shows a submenu of configured hosts;
          Pull back appears only when the workspace is currently
          remote. Both fall back to "go to Settings" when no hosts
          are configured. */}
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
    // Clear any active draft first so `WorkspaceMain`'s lazy branch
    // (`lazyEnabled && activeDraftId`) doesn't keep the draft surface
    // on screen after the backend's active_workspace_id flips. The
    // draft itself stays in the store — clicking "+" again restores
    // it with composer text intact via the single-slot rule.
    useChatDraftStore.getState().setActiveDraft(null);
    // Wrap in `startTransition` so React treats the workspace switch
    // as a non-urgent update. The expensive commit (unmount the old
    // workspace's pane tree, mount the new one — including 2-3
    // TerminalPane mount effects with their xterm setup + IPC
    // round-trips, possibly an EditorPane CodeMirror rebuild) still
    // happens, but React keeps yielding to the browser between fiber
    // units while it's running so the click feedback, hover states,
    // and any in-flight typing remain responsive instead of feeling
    // frozen. See react.dev/reference/react/startTransition. Pairs
    // with the backend `spawn_missing_ptys_for_workspace` move off
    // the IPC thread and the parallelised mount IPCs.
    startTransition(() => {
      activateWorkspace(workspace.workspace_id).catch(console.error);
    });
  };

  const isPrimary = !workspace.worktree_path;
  // Default branches (no worktree_path — the primary project checkout,
  // usually main/master) can't be deleted. We still offer a Hide path
  // via right-click → Close Worktree, but the inline X is redundant
  // and the button's Hide-only dialog feels like a ghost click. Match
  // Cursor 3: hide the X on these rows.
  const canDelete = !isPrimary;
  // Icon picks up the workspace's location: Cloud for remote
  // workspaces (host_id set), Loader2 while a push/pull is in flight,
  // Workflow for OpenFlow workspaces, Laptop for the primary
  // checkout, GitBranch for local branch workspaces. The remote
  // disconnect indicator (CloudOff in muted color) lands when the
  // TunnelSupervisor's status feed is wired into this row — for now
  // a remote workspace is just "Cloud."
  const isRemote =
    workspace.host_id !== null && workspace.host_id !== undefined;
  const isPushOrPullInFlight = useAppStore(
    (s) => s.workspacePushPullInFlight === workspace.workspace_id,
  );
  const icon = isPushOrPullInFlight ? (
    <Loader2 className="h-4 w-4 shrink-0 text-muted-foreground animate-spin" />
  ) : isRemote ? (
    <Cloud className="h-4 w-4 shrink-0 text-muted-foreground" />
  ) : workspace.workspace_type === "open_flow" ? (
    <Workflow className="h-4 w-4 shrink-0 text-muted-foreground" />
  ) : isPrimary ? (
    <Laptop className="h-4 w-4 shrink-0 text-muted-foreground" />
  ) : (
    <GitBranch className="h-4 w-4 shrink-0 text-muted-foreground" />
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
              "flex w-full pl-3 pr-2 py-1.5 text-sm cursor-pointer group relative",
              "hover:bg-muted/50 transition-colors",
              isActive && "bg-muted",
            )}
          >
            {/* Active left border accent */}
            {isActive && (
              <div className="absolute left-0 top-0 bottom-0 w-0.5 bg-foreground rounded-r" />
            )}

            {/* Icon — size-6 container matches project header avatar width */}
            <div className="relative size-6 flex items-center justify-center shrink-0 mr-2.5">
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

            {/* Content */}
            <div className="flex-1 min-w-0">
              <div className="flex items-center gap-1.5">
                <span className={cn(
                  "truncate text-[13px] leading-tight font-medium",
                  isActive ? "text-foreground" : "text-foreground/80",
                )}>
                  {workspace.title}
                </span>

                {/* Remote workspaces are signalled by the leading
                    Cloud icon (set above). No badge here — the icon
                    swap is the indicator. */}

                {/* Ahead/behind indicators */}
                {(workspace.git_ahead > 0 || workspace.git_behind > 0) && (
                  <span className="flex items-center gap-1 shrink-0 text-[10px] font-mono tabular-nums">
                    {workspace.git_behind > 0 && (
                      <span className="text-warning">↓{workspace.git_behind}</span>
                    )}
                    {workspace.git_ahead > 0 && (
                      <span className="text-success">↑{workspace.git_ahead}</span>
                    )}
                  </span>
                )}

                {/* Git diff stats — in normal flow; fades on hover
                    (only when the X is going to appear) so the
                    absolute-positioned X doesn't overlap numbers. */}
                {(workspace.git_additions > 0 || workspace.git_deletions > 0) && (
                  <span className={cn(
                    "ml-auto flex items-center gap-1.5 text-[10px] font-mono tabular-nums rounded px-1.5 h-5 shrink-0",
                    isActive ? "bg-foreground/10" : "bg-muted/50",
                    canDelete && "transition-opacity group-hover:opacity-0",
                  )}>
                    {workspace.git_additions > 0 && (
                      <span className="text-success">+{workspace.git_additions}</span>
                    )}
                    {workspace.git_deletions > 0 && (
                      <span className="text-danger">−{workspace.git_deletions}</span>
                    )}
                  </span>
                )}

                {/* Notification badge — pinned to the far right in
                    normal state. Fades on hover when the X will
                    appear so they don't collide on the same slot. */}
                {workspace.notification_count > 0 && (
                  <Badge
                    variant="outline"
                    className={cn(
                      "shrink-0 text-[10px] tabular-nums text-warning bg-warning/20 border-transparent px-1.5 py-0.5 leading-none",
                      // `ml-auto` only when the git-diff pill isn't
                      // holding that slot; otherwise inherit the
                      // default flex spacing after the git-diff pill.
                      !(workspace.git_additions > 0 || workspace.git_deletions > 0) && "ml-auto",
                      canDelete && "transition-opacity group-hover:opacity-0",
                    )}
                  >
                    {workspace.notification_count}
                  </Badge>
                )}
              </div>

              {/* Branch name row. Issue + PR badges are grouped into a
                  single right-aligned cluster so they sit next to each
                  other at the bottom-right when both are present (no dead
                  gap), and the branch text is the only element that
                  truncates when space is tight. */}
              {workspace.git_branch && (
                <div className="flex items-center gap-1 text-[11px] text-muted-foreground/60 font-mono leading-tight mt-0.5">
                  <span className="truncate">{workspace.git_branch}</span>
                  {(workspace.linked_issue || showPrIcon || workspace.notifications_muted) && (
                    <div className="flex items-center gap-1 ml-auto shrink-0">
                      {/* Muted indicator — only shown when muted; absence
                          means notifications are on (the normal state). */}
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

            {/* Remove button — absolute-positioned at a fixed
                right offset so the hover-reveal X sits at the same
                x-coordinate on every row, regardless of what mix of
                git-diff pill / notification badge / PR badge sits in
                the normal flow. Sibling badges fade on hover (above)
                so the overlay is visually exclusive. Only rendered
                for worktrees with a worktree_path — the primary
                checkout is Hide-only via right-click → Close
                Worktree. */}
            {canDelete && (
              <Button
                variant="ghost"
                size="icon-xs"
                className="absolute right-2 top-1/2 -translate-y-1/2 opacity-0 group-hover:opacity-100 transition-opacity text-muted-foreground hover:text-foreground"
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
