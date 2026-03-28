import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { FolderPlus, FolderOpen, GitFork } from "lucide-react";
import { useProjectActions } from "@/hooks/use-project-actions";

export function SidebarAddRepo() {
  const { openProject, openCloneDialog } = useProjectActions();

  return (
    <div className="border-t border-border p-2">
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="sm"
            className="w-full justify-start gap-2 text-muted-foreground hover:text-foreground"
          >
            <FolderPlus className="h-4 w-4" />
            Add repository
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent side="top" align="start">
          <DropdownMenuItem onClick={() => openProject()} className="text-xs">
            <FolderOpen className="mr-2 h-3.5 w-3.5" />
            Open project
          </DropdownMenuItem>
          <DropdownMenuItem onClick={openCloneDialog} className="text-xs">
            <GitFork className="mr-2 h-3.5 w-3.5" />
            Clone repository
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}
