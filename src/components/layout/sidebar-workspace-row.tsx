import { useEffect, useState } from "react";
import {
  SidebarMenuItem,
  SidebarMenuButton,
  SidebarMenuBadge,
  SidebarMenuAction,
  SidebarMenuSubButton,
} from "@/components/ui/sidebar";
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
import { Badge } from "@/components/ui/badge";
import { X, TerminalSquare, Workflow } from "lucide-react";
import {
  activateWorkspace,
  closeWorkspace,
  renameWorkspace,
  detectEditors,
  openInEditor,
} from "@/tauri/commands";
import type { WorkspaceSnapshot, EditorInfo } from "@/tauri/types";

interface Props {
  workspace: WorkspaceSnapshot;
  isActive: boolean;
  nested?: boolean;
}

function WorkspaceRowContent({ workspace }: { workspace: WorkspaceSnapshot }) {
  return (
    <div className="flex flex-col gap-0 min-w-0 flex-1">
      <div className="flex items-center justify-between gap-2">
        <span className="truncate text-sm font-medium leading-tight">{workspace.title}</span>
        {(workspace.git_additions > 0 || workspace.git_deletions > 0) && (
          <span className="flex items-center gap-1 shrink-0 text-[10px] tabular-nums">
            {workspace.git_additions > 0 && (
              <span className="text-success">+{workspace.git_additions}</span>
            )}
            {workspace.git_deletions > 0 && (
              <span className="text-danger">-{workspace.git_deletions}</span>
            )}
          </span>
        )}
      </div>
      {workspace.git_branch && (
        <div className="flex items-center gap-1 text-[10px] text-muted-foreground leading-tight">
          <span className="truncate">{workspace.git_branch}</span>
          {workspace.pr_number && (
            <Badge variant="outline" className="h-3.5 px-1 text-[9px] leading-none">
              #{workspace.pr_number}
            </Badge>
          )}
        </div>
      )}
    </div>
  );
}

function WorkspaceContextMenuItems({ workspace }: { workspace: WorkspaceSnapshot }) {
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
        onClick={() => closeWorkspace(workspace.workspace_id, false).catch(console.error)}
      >
        Close workspace
      </ContextMenuItem>
      <ContextMenuItem
        className="text-destructive focus:text-destructive"
        onClick={() => closeWorkspace(workspace.workspace_id, true).catch(console.error)}
      >
        Delete workspace
      </ContextMenuItem>
    </ContextMenuContent>
  );
}

export function SidebarWorkspaceRow({ workspace, isActive, nested }: Props) {
  const handleActivate = () => {
    activateWorkspace(workspace.workspace_id).catch(console.error);
  };

  const handleClose = (e: React.MouseEvent) => {
    e.stopPropagation();
    closeWorkspace(workspace.workspace_id, false).catch(console.error);
  };

  const icon =
    workspace.workspace_type === "open_flow" ? (
      <Workflow className="h-4 w-4 shrink-0 text-muted-foreground" />
    ) : (
      <TerminalSquare className="h-4 w-4 shrink-0 text-muted-foreground" />
    );

  if (nested) {
    return (
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <SidebarMenuSubButton
            isActive={isActive}
            onClick={handleActivate}
          >
            {icon}
            <WorkspaceRowContent workspace={workspace} />
          </SidebarMenuSubButton>
        </ContextMenuTrigger>
        <WorkspaceContextMenuItems workspace={workspace} />
      </ContextMenu>
    );
  }

  return (
    <SidebarMenuItem>
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <SidebarMenuButton
            isActive={isActive}
            onClick={handleActivate}
            tooltip={workspace.title}
            size="lg"
          >
            {icon}
            <WorkspaceRowContent workspace={workspace} />
          </SidebarMenuButton>
        </ContextMenuTrigger>
        <WorkspaceContextMenuItems workspace={workspace} />
      </ContextMenu>
      {workspace.notification_count > 0 && (
        <SidebarMenuBadge className="bg-warning/20 text-warning">
          {workspace.notification_count}
        </SidebarMenuBadge>
      )}
      <SidebarMenuAction showOnHover onClick={handleClose} title="Close workspace">
        <X className="h-3 w-3" />
      </SidebarMenuAction>
    </SidebarMenuItem>
  );
}
