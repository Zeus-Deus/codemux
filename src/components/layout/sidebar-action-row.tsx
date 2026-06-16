import { SidebarHeader as ShadcnSidebarHeader, SidebarSeparator, useSidebar } from "@/components/ui/sidebar";
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
import { Plus, FolderPlus, FolderOpen, CalendarClock, LayoutGrid } from "lucide-react";
import { useProjectActions } from "@/hooks/use-project-actions";

export function SidebarActionRow() {
  const { state } = useSidebar();
  const setShowNewWorkspaceDialog = useUIStore((s) => s.setShowNewWorkspaceDialog);
  const setShowNewProjectScreen = useUIStore((s) => s.setShowNewProjectScreen);
  const setShowAutomations = useUIStore((s) => s.setShowAutomations);
  const setShowWorkspacesOverview = useUIStore((s) => s.setShowWorkspacesOverview);
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

  // Collapsed icon rail: the same actions as vertical icon buttons, each
  // labelled by a right-side tooltip. New agent is accented as the primary
  // create action; Add repository keeps its dropdown (opening to the right).
  if (state === "collapsed") {
    return (
      <ShadcnSidebarHeader className="gap-0 p-0">
        <div className="flex flex-col items-center gap-1 px-1 py-2">
          <Tooltip delayDuration={300}>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon"
                aria-label="New agent"
                onClick={handleNewAgent}
                className="size-8 text-primary hover:bg-primary/10 hover:text-primary"
              >
                <Plus className="size-[18px]" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="right" className="text-xs">
              New agent · Shift+click for workspace dialog
            </TooltipContent>
          </Tooltip>

          <DropdownMenu>
            <Tooltip delayDuration={300}>
              <TooltipTrigger asChild>
                <DropdownMenuTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon"
                    aria-label="Add repository"
                    className="size-8 text-muted-foreground hover:bg-sidebar-accent hover:text-sidebar-accent-foreground"
                  >
                    <FolderPlus className="size-[18px]" />
                  </Button>
                </DropdownMenuTrigger>
              </TooltipTrigger>
              <TooltipContent side="right" className="text-xs">
                Add repository
              </TooltipContent>
            </Tooltip>
            <DropdownMenuContent side="right" align="start">
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

          <Tooltip delayDuration={300}>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon"
                aria-label="Automations"
                onClick={() => setShowAutomations(true)}
                className="size-8 text-muted-foreground hover:bg-sidebar-accent hover:text-sidebar-accent-foreground"
              >
                <CalendarClock className="size-[18px]" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="right" className="text-xs">
              Automations
            </TooltipContent>
          </Tooltip>

          <Tooltip delayDuration={300}>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon"
                aria-label="Workspaces"
                onClick={() => setShowWorkspacesOverview(true)}
                className="size-8 text-muted-foreground hover:bg-sidebar-accent hover:text-sidebar-accent-foreground"
              >
                <LayoutGrid className="size-[18px]" />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="right" className="text-xs">
              Workspaces
            </TooltipContent>
          </Tooltip>
        </div>
        <SidebarSeparator />
      </ShadcnSidebarHeader>
    );
  }

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

      {/* Automations — a first-class destination under "New agent",
          above the project list, matching where Codex and Superset
          place it. Opens the full-screen Automations view. */}
      <div className="px-2 pb-1">
        <Button
          variant="ghost"
          aria-label="Automations"
          className="w-full justify-start gap-2 h-8 px-2 text-[13px] text-muted-foreground hover:bg-sidebar-accent hover:text-sidebar-accent-foreground"
          onClick={() => setShowAutomations(true)}
        >
          <CalendarClock className="size-[18px]" />
          <span>Automations</span>
        </Button>
      </div>

      {/* Workspaces — a single pane that lists every workspace this
          device knows about (local + every host it has pushed to),
          with filters, search, and per-row push/pull/open actions.
          Same full-screen overlay shape as Automations. */}
      <div className="px-2 pb-1.5">
        <Button
          variant="ghost"
          aria-label="Workspaces"
          className="w-full justify-start gap-2 h-8 px-2 text-[13px] text-muted-foreground hover:bg-sidebar-accent hover:text-sidebar-accent-foreground"
          onClick={() => setShowWorkspacesOverview(true)}
        >
          <LayoutGrid className="size-[18px]" />
          <span>Workspaces</span>
        </Button>
      </div>

      <SidebarSeparator />
    </ShadcnSidebarHeader>
  );
}
