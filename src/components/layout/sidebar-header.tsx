import { SidebarHeader as ShadcnSidebarHeader } from "@/components/ui/sidebar";
import { useActiveWorkspace } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { GitBranch, Plus, FolderPlus, Settings } from "lucide-react";

export function SidebarHeader() {
  const activeWorkspace = useActiveWorkspace();
  const setShowSettings = useUIStore((s) => s.setShowSettings);

  return (
    <ShadcnSidebarHeader className="gap-1 p-3">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <span className="inline-block h-2.5 w-2.5 rotate-45 rounded-sm bg-primary shadow-[0_0_8px_var(--primary)]" />
          <span className="text-sm font-bold tracking-wide text-foreground">
            Codemux
          </span>
        </div>
        <div className="flex items-center gap-1">
          <button
            className="inline-flex h-6 w-6 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"
            title="New section"
          >
            <FolderPlus className="h-3.5 w-3.5" />
          </button>
          <button
            className="inline-flex h-6 w-6 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"
            title="New workspace"
          >
            <Plus className="h-3.5 w-3.5" />
          </button>
          <button
            className="inline-flex h-6 w-6 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"
            title="Settings"
            onClick={() => setShowSettings(true)}
          >
            <Settings className="h-3.5 w-3.5" />
          </button>
        </div>
      </div>
      {activeWorkspace && (
        <div className="mt-1 space-y-0.5">
          <p className="truncate text-sm font-medium text-foreground">
            {activeWorkspace.title}
          </p>
          {activeWorkspace.git_branch && (
            <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
              <GitBranch className="h-3 w-3" />
              <span className="truncate">{activeWorkspace.git_branch}</span>
              {activeWorkspace.git_additions > 0 && (
                <span className="text-success">+{activeWorkspace.git_additions}</span>
              )}
              {activeWorkspace.git_deletions > 0 && (
                <span className="text-danger">-{activeWorkspace.git_deletions}</span>
              )}
            </div>
          )}
        </div>
      )}
    </ShadcnSidebarHeader>
  );
}
