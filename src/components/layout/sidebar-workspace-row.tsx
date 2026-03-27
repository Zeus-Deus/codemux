import { useEffect, useState } from "react";
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
import { X, TerminalSquare, Workflow, ArrowUp, ArrowDown } from "lucide-react";
import {
  activateWorkspace,
  closeWorkspace,
  closeWorkspaceWithWorktree,
  renameWorkspace,
  detectEditors,
  openInEditor,
} from "@/tauri/commands";
import type { WorkspaceSnapshot, EditorInfo } from "@/tauri/types";

interface Props {
  workspace: WorkspaceSnapshot;
  isActive: boolean;
}

function handleCloseWorkspace(workspace: WorkspaceSnapshot) {
  if (workspace.worktree_path) {
    closeWorkspaceWithWorktree(workspace.workspace_id, true, false, false).catch(console.error);
  } else {
    closeWorkspace(workspace.workspace_id, false).catch(console.error);
  }
}

function DeleteWorkspaceDialog({
  workspace,
  open,
  onOpenChange,
}: {
  workspace: WorkspaceSnapshot;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const isWorktree = !!workspace.worktree_path;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-sm">
        <DialogHeader>
          <DialogTitle>Delete workspace &ldquo;{workspace.title}&rdquo;?</DialogTitle>
          <DialogDescription>
            {isWorktree && workspace.git_branch
              ? `This will remove the worktree directory and delete the git branch '${workspace.git_branch}'.`
              : "This will close the workspace."}
          </DialogDescription>
        </DialogHeader>
        <div className="flex justify-end gap-2 pt-2">
          <Button variant="outline" size="sm" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          {isWorktree ? (
            <Button
              variant="destructive"
              size="sm"
              onClick={() => {
                closeWorkspaceWithWorktree(workspace.workspace_id, true, true, false).catch(console.error);
                onOpenChange(false);
              }}
            >
              Delete
            </Button>
          ) : (
            <Button
              variant="destructive"
              size="sm"
              onClick={() => {
                closeWorkspace(workspace.workspace_id, true).catch(console.error);
                onOpenChange(false);
              }}
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
  onDeleteRequest,
}: {
  workspace: WorkspaceSnapshot;
  onDeleteRequest: () => void;
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
      <ContextMenuSeparator />
      <ContextMenuItem
        onClick={() => handleCloseWorkspace(workspace)}
      >
        Close workspace
      </ContextMenuItem>
      <ContextMenuItem
        className="text-destructive focus:text-destructive"
        onClick={onDeleteRequest}
      >
        Delete workspace
      </ContextMenuItem>
    </ContextMenuContent>
  );
}

export function SidebarWorkspaceRow({ workspace, isActive }: Props) {
  const [showDeleteDialog, setShowDeleteDialog] = useState(false);

  const handleActivate = () => {
    activateWorkspace(workspace.workspace_id).catch(console.error);
  };

  const icon =
    workspace.workspace_type === "open_flow" ? (
      <Workflow className="h-4 w-4 shrink-0 text-muted-foreground" />
    ) : (
      <TerminalSquare className="h-4 w-4 shrink-0 text-muted-foreground" />
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
              <div className="absolute left-0 top-0 bottom-0 w-0.5 bg-primary rounded-r" />
            )}

            {/* Icon */}
            <div className="flex items-center shrink-0 mr-2.5 mt-0.5">
              {icon}
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

                {/* Git diff stats — hidden on hover, replaced by close button */}
                {(workspace.git_additions > 0 || workspace.git_deletions > 0) && (
                  <span className="flex items-center gap-1 shrink-0 text-[10px] font-mono tabular-nums transition-opacity group-hover:opacity-0">
                    {workspace.git_additions > 0 && (
                      <span className="text-success">+{workspace.git_additions}</span>
                    )}
                    {workspace.git_deletions > 0 && (
                      <span className="text-danger">-{workspace.git_deletions}</span>
                    )}
                  </span>
                )}

                {/* Close button — visible on hover */}
                <button
                  type="button"
                  onClick={(e) => { e.stopPropagation(); handleCloseWorkspace(workspace); }}
                  className="ml-auto shrink-0 opacity-0 group-hover:opacity-100 transition-opacity text-muted-foreground hover:text-foreground"
                  aria-label="Close workspace"
                >
                  <X className="h-3.5 w-3.5" />
                </button>

                {/* Notification badge */}
                {workspace.notification_count > 0 && (
                  <span className="shrink-0 text-[10px] tabular-nums text-warning bg-warning/20 rounded-full px-1.5 py-0.5 leading-none">
                    {workspace.notification_count}
                  </span>
                )}
              </div>

              {/* Branch name row */}
              {workspace.git_branch && (
                <div className="flex items-center gap-1 text-[11px] text-muted-foreground/60 font-mono leading-tight mt-0.5">
                  <span className="truncate">{workspace.git_branch}</span>
                  {(workspace.git_ahead > 0 || workspace.git_behind > 0) && (
                    <span className="flex items-center gap-0.5 shrink-0 text-[10px] tabular-nums">
                      {workspace.git_ahead > 0 && (
                        <span className="flex items-center text-success"><ArrowUp className="h-2 w-2" />{workspace.git_ahead}</span>
                      )}
                      {workspace.git_behind > 0 && (
                        <span className="flex items-center text-warning"><ArrowDown className="h-2 w-2" />{workspace.git_behind}</span>
                      )}
                    </span>
                  )}
                  {workspace.pr_number && (
                    <Badge variant="outline" className="h-3.5 px-1 text-[9px] leading-none">
                      #{workspace.pr_number}
                    </Badge>
                  )}
                </div>
              )}
            </div>
          </div>
        </ContextMenuTrigger>
        <WorkspaceContextMenuItems
          workspace={workspace}
          onDeleteRequest={() => setShowDeleteDialog(true)}
        />
      </ContextMenu>
      <DeleteWorkspaceDialog
        workspace={workspace}
        open={showDeleteDialog}
        onOpenChange={setShowDeleteDialog}
      />
    </>
  );
}
