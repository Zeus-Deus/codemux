import {
  SidebarMenuItem,
  SidebarMenuButton,
  SidebarMenuBadge,
  SidebarMenuAction,
  SidebarMenuSubButton,
} from "@/components/ui/sidebar";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";
import { X, GitBranch } from "lucide-react";
import { activateWorkspace, closeWorkspace } from "@/tauri/commands";
import type { WorkspaceSnapshot } from "@/tauri/types";

interface Props {
  workspace: WorkspaceSnapshot;
  isActive: boolean;
  nested?: boolean;
}

function WorkspaceRowContent({ workspace }: { workspace: WorkspaceSnapshot }) {
  return (
    <div className="flex flex-col gap-0.5 min-w-0">
      <span className="truncate font-medium">{workspace.title}</span>
      {workspace.git_branch && (
        <div className="flex items-center gap-1 text-[11px] text-muted-foreground">
          <GitBranch className="h-2.5 w-2.5 shrink-0" />
          <span className="truncate">{workspace.git_branch}</span>
          {workspace.pr_number && (
            <Badge variant="outline" className="h-4 px-1 text-[10px]">
              #{workspace.pr_number}
            </Badge>
          )}
          {(workspace.git_additions > 0 || workspace.git_deletions > 0) && (
            <span className="ml-auto flex items-center gap-1 shrink-0 tabular-nums">
              {workspace.git_additions > 0 && (
                <span className="text-success">+{workspace.git_additions}</span>
              )}
              {workspace.git_deletions > 0 && (
                <span className="text-danger">-{workspace.git_deletions}</span>
              )}
            </span>
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

  const statusDotColor =
    workspace.notification_count > 0
      ? "bg-warning"
      : isActive
        ? "bg-primary"
        : "bg-muted-foreground/40";

  // Nested rows are already inside SidebarMenuSubItem (<li>),
  // so use SidebarMenuSubButton instead of SidebarMenuItem + SidebarMenuButton.
  if (nested) {
    return (
      <SidebarMenuSubButton
        isActive={isActive}
        onClick={handleActivate}
        className={
          isActive
            ? "border-l-2 border-l-primary bg-sidebar-accent"
            : "border-l-2 border-l-transparent"
        }
      >
        <span
          className={cn("inline-block h-1.5 w-1.5 shrink-0 rounded-full", statusDotColor)}
        />
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
        size="default"
        className={
          isActive
            ? "border-l-2 border-l-primary bg-sidebar-accent"
            : "border-l-2 border-l-transparent"
        }
      >
        <span
          className={cn("inline-block h-1.5 w-1.5 shrink-0 rounded-full", statusDotColor)}
        />
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
