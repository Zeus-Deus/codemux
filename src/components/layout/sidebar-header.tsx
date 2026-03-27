import { SidebarHeader as ShadcnSidebarHeader, SidebarSeparator } from "@/components/ui/sidebar";
import { Button } from "@/components/ui/button";
import { useUIStore } from "@/stores/ui-store";
import { Plus } from "lucide-react";

export function SidebarHeader() {
  const setShowDialog = useUIStore((s) => s.setShowNewWorkspaceDialog);

  return (
    <ShadcnSidebarHeader className="gap-0 p-2 pb-0">
      {/* + New Workspace row */}
      <Button
        variant="ghost"
        className="w-full justify-start pl-3 pr-2 py-3 h-auto text-muted-foreground hover:bg-sidebar-accent hover:text-sidebar-accent-foreground"
        onClick={() => setShowDialog(true)}
      >
        <div className="size-6 flex items-center justify-center shrink-0 mr-2.5">
          <Plus className="h-3.5 w-3.5" />
        </div>
        <span>New Workspace</span>
      </Button>
      <SidebarSeparator />
    </ShadcnSidebarHeader>
  );
}
