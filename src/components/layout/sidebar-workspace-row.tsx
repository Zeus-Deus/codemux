import {
  SidebarMenuItem,
  SidebarMenuButton,
  SidebarMenuBadge,
  SidebarMenuAction,
  SidebarMenuSubButton,
} from "@/components/ui/sidebar";
import { Badge } from "@/components/ui/badge";
import { X, TerminalSquare, Workflow } from "lucide-react";
import { activateWorkspace, closeWorkspace } from "@/tauri/commands";
import type { WorkspaceSnapshot } from "@/tauri/types";

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

  // Nested rows are already inside SidebarMenuSubItem (<li>),
  // so use SidebarMenuSubButton instead of SidebarMenuItem + SidebarMenuButton.
  if (nested) {
    return (
      <SidebarMenuSubButton
        isActive={isActive}
        onClick={handleActivate}
      >
        {icon}
        <WorkspaceRowContent workspace={workspace} />
      </SidebarMenuSubButton>
    );
  }

  return (
    <SidebarMenuItem>
      <SidebarMenuButton
        isActive={isActive}
        onClick={handleActivate}
        tooltip={workspace.title}
        size="lg"
      >
        {icon}
        <WorkspaceRowContent workspace={workspace} />
      </SidebarMenuButton>
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
