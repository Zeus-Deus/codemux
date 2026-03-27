import { SidebarHeader as ShadcnSidebarHeader } from "@/components/ui/sidebar";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { Plus } from "lucide-react";

export function SidebarHeader() {
  const workspaceCount = useAppStore((s) => s.appState?.workspaces.length ?? 0);
  const activeWorkspace = useAppStore(
    (s) => s.appState?.workspaces.find((w) => w.workspace_id === s.appState?.active_workspace_id),
  );
  const setShowDialog = useUIStore((s) => s.setShowNewWorkspaceDialog);

  // Derive repo name from active workspace cwd, fallback to "Workspaces"
  const repoName = activeWorkspace?.cwd
    ? activeWorkspace.cwd.split("/").filter(Boolean).pop() ?? "Workspaces"
    : "Workspaces";

  return (
    <ShadcnSidebarHeader className="gap-0 p-2">
      {/* + New Workspace row */}
      <Button
        variant="ghost"
        className="w-full justify-start gap-2 px-2 py-3 h-auto text-muted-foreground hover:bg-sidebar-accent hover:text-sidebar-accent-foreground"
        onClick={() => setShowDialog(true)}
      >
        <Plus className="h-3.5 w-3.5" />
        <span>New Workspace</span>
      </Button>

      {/* Repo name + count + actions */}
      <div className="flex items-center justify-between px-2 py-1">
        <div className="flex items-center gap-1.5 min-w-0">
          <span className="truncate text-sm font-semibold text-foreground">
            {repoName}
          </span>
          <span className="text-xs text-muted-foreground/60 tabular-nums shrink-0">
            ({workspaceCount})
          </span>
        </div>
        <div className="flex items-center gap-0.5 shrink-0">
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label="New workspace"
                onClick={() => setShowDialog(true)}
              >
                <Plus className="h-3 w-3" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="bottom" sideOffset={4}>
              New workspace
            </TooltipContent>
          </Tooltip>
        </div>
      </div>
    </ShadcnSidebarHeader>
  );
}
