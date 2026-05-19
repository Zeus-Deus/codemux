import { SidebarHeader as ShadcnSidebarHeader, SidebarSeparator } from "@/components/ui/sidebar";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useUIStore } from "@/stores/ui-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { Plus, FolderPlus, FolderOpen } from "lucide-react";
import { useProjectActions } from "@/hooks/use-project-actions";

export function SidebarActionRow() {
  const setShowNewWorkspaceDialog = useUIStore((s) => s.setShowNewWorkspaceDialog);
  const setShowNewProjectScreen = useUIStore((s) => s.setShowNewProjectScreen);
  const enableAgentChat = useFeatureFlags((s) => s.enableAgentChat);
  const enableLazyWorkspaceCreation = useFeatureFlags(
    (s) => s.enableLazyWorkspaceCreation,
  );
  const { openProject } = useProjectActions();

  const handleNewAgent = (e: React.MouseEvent) => {
    if (e.shiftKey || !enableAgentChat) {
      setShowNewWorkspaceDialog(true);
      return;
    }
    if (enableLazyWorkspaceCreation) {
      const store = useChatDraftStore.getState();
      // `lockedToHome: true` opts the resulting draft out of
      // `DraftChatSurface`'s mount-time auto-seed and submit-time
      // salvage, both of which would otherwise redirect the draft to
      // whatever project workspace happens to be active in the
      // sidebar. The tooltip on this button promises "New chat in
      // home directory", so we honour that literally.
      const draft = store.getOrCreateHomeDraft({ lockedToHome: true });
      store.setActiveDraft(draft.draftId);
      return;
    }
    setShowNewWorkspaceDialog(true);
  };

  return (
    <ShadcnSidebarHeader className="gap-0 p-0">
      <div className="flex items-center gap-1 px-2 py-1.5">
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              variant="ghost"
              aria-label="New agent"
              className="flex-1 justify-start gap-2 h-8 px-2 text-[13px] text-muted-foreground hover:bg-sidebar-accent hover:text-sidebar-accent-foreground"
              onClick={handleNewAgent}
            >
              <Plus className="size-[18px]" />
              <span>New agent</span>
            </Button>
          </TooltipTrigger>
          <TooltipContent side="bottom" sideOffset={4} className="text-xs">
            New chat in home directory · Shift+click for workspace dialog
          </TooltipContent>
        </Tooltip>

        <DropdownMenu>
          <Tooltip>
            <TooltipTrigger asChild>
              <DropdownMenuTrigger asChild>
                <Button
                  variant="ghost"
                  size="icon-xs"
                  aria-label="Add repository"
                  className="h-8 w-8 text-muted-foreground hover:bg-sidebar-accent hover:text-sidebar-accent-foreground"
                >
                  <FolderPlus className="size-[18px] text-muted-foreground group-hover/button:text-sidebar-accent-foreground" />
                </Button>
              </DropdownMenuTrigger>
            </TooltipTrigger>
            <TooltipContent side="bottom" sideOffset={4} className="text-xs">
              Add repository
            </TooltipContent>
          </Tooltip>
          <DropdownMenuContent side="bottom" align="end">
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
      </div>
      <SidebarSeparator />
    </ShadcnSidebarHeader>
  );
}
