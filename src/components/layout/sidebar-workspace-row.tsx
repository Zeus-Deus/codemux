import { useEffect, useRef, useState } from "react";
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
import { cn } from "@/lib/utils";
import { X, Laptop, GitBranch, Workflow, AlertTriangle } from "lucide-react";
import {
  activateWorkspace,
  closeWorkspace,
  closeWorkspaceWithWorktree,
  renameWorkspace,
  detectEditors,
  openInEditor,
  runWorkspaceSetup,
} from "@/tauri/commands";
import type { WorkspaceSnapshot, EditorInfo, ActivePaneStatus } from "@/tauri/types";
import { useAppStore } from "@/stores/app-store";
import { getWorkspaceStatus } from "@/lib/pane-status";
import { StatusIndicator } from "@/components/ui/status-indicator";
import { IssueDetailPopover } from "@/components/github/issue-detail-popover";

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
            Remove workspace &ldquo;{workspace.title}&rdquo;?
          </DialogTitle>
          <DialogDescription>
            {isWorktree
              ? "Deleting will permanently remove the worktree. You can hide instead to keep files on disk."
              : "This will close the workspace."}
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
          <Button
            variant="secondary"
            size="sm"
            className="h-7 px-3 text-xs"
            onClick={handleHide}
          >
            Hide
          </Button>
          {isWorktree && (
            <Button
              variant="destructive"
              size="sm"
              className="h-7 px-3 text-xs"
              onClick={handleDelete}
            >
              Delete
            </Button>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}

function WorkspaceContextMenuItems({
  workspace,
  onRemoveRequest,
}: {
  workspace: WorkspaceSnapshot;
  onRemoveRequest: () => void;
}) {
  const [editors, setEditors] = useState<EditorInfo[]>([]);

  useEffect(() => {
    detectEditors().then(setEditors).catch(console.error);
  }, []);

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

  const handleOpenInEditor = (editorId: string) => {
    openInEditor(editorId, workspace.cwd).catch(console.error);
  };

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
      <ContextMenuItem
        onClick={() => runWorkspaceSetup(workspace.workspace_id).catch(console.error)}
      >
        Re-run Setup
      </ContextMenuItem>
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
    activateWorkspace(workspace.workspace_id).catch(console.error);
  };

  const isPrimary = !workspace.worktree_path;
  const icon =
    workspace.workspace_type === "open_flow" ? (
      <Workflow className="h-4 w-4 shrink-0 text-muted-foreground" />
    ) : isPrimary ? (
      <Laptop className="h-4 w-4 shrink-0 text-muted-foreground" />
    ) : (
      <GitBranch className="h-4 w-4 shrink-0 text-muted-foreground" />
    );

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

                {/* Git diff stats + close button overlay */}
                <div className="ml-auto grid shrink-0 [&>*]:col-start-1 [&>*]:row-start-1 items-center">
                  {(workspace.git_additions > 0 || workspace.git_deletions > 0) && (
                    <span className={cn(
                      "flex items-center gap-1.5 text-[10px] font-mono tabular-nums rounded px-1.5 h-5",
                      isActive ? "bg-foreground/10" : "bg-muted/50",
                      "transition-opacity group-hover:opacity-0",
                    )}>
                      {workspace.git_additions > 0 && (
                        <span className="text-success">+{workspace.git_additions}</span>
                      )}
                      {workspace.git_deletions > 0 && (
                        <span className="text-danger">−{workspace.git_deletions}</span>
                      )}
                    </span>
                  )}
                  <Button
                    variant="ghost"
                    size="icon-xs"
                    className="opacity-0 group-hover:opacity-100 transition-opacity text-muted-foreground hover:text-foreground justify-end"
                    onClick={(e) => { e.stopPropagation(); setShowRemoveDialog(true); }}
                    aria-label="Remove workspace"
                  >
                    <X className="h-3.5 w-3.5" />
                  </Button>
                </div>

                {/* Notification badge */}
                {workspace.notification_count > 0 && (
                  <Badge variant="outline" className="shrink-0 text-[10px] tabular-nums text-warning bg-warning/20 border-transparent px-1.5 py-0.5 leading-none">
                    {workspace.notification_count}
                  </Badge>
                )}
              </div>

              {/* Branch name row */}
              {workspace.git_branch && (
                <div className="flex items-center gap-1 text-[11px] text-muted-foreground/60 font-mono leading-tight mt-0.5">
                  <span className="truncate">{workspace.git_branch}</span>
                  {workspace.pr_number && (
                    <Badge variant="outline" className="h-3.5 px-1 text-[9px] leading-none">
                      #{workspace.pr_number}
                    </Badge>
                  )}
                  {workspace.linked_issue && (
                    <IssueDetailPopover
                      workspaceId={workspace.workspace_id}
                      issue={workspace.linked_issue}
                    />
                  )}
                </div>
              )}
            </div>
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
