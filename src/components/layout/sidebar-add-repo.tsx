import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { SidebarSeparator } from "@/components/ui/sidebar";
import { FolderPlus, FolderOpen } from "lucide-react";
import { useProjectActions } from "@/hooks/use-project-actions";
import { useUIStore } from "@/stores/ui-store";

export function SidebarAddRepo() {
  const { openProject } = useProjectActions();
  const setShowNewProjectScreen = useUIStore(
    (s) => s.setShowNewProjectScreen,
  );

  return (
    <>
      <SidebarSeparator />
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            className="w-full justify-start pl-3 pr-2 py-3 h-auto text-muted-foreground hover:bg-sidebar-accent hover:text-sidebar-accent-foreground"
          >
            <div className="size-6 flex items-center justify-center shrink-0 mr-2.5">
              <FolderPlus className="h-3.5 w-3.5" />
            </div>
            <span>Add repository</span>
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent side="top" align="start">
          <DropdownMenuItem onClick={() => openProject()} className="text-xs">
            <FolderOpen className="mr-2 h-3.5 w-3.5" />
            Open project
          </DropdownMenuItem>
          <DropdownMenuItem
            onClick={() => setShowNewProjectScreen(true)}
            className="text-xs"
          >
            <FolderPlus className="mr-2 h-3.5 w-3.5" />
            New project
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </>
  );
}
