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
import {
  Plus,
  FolderPlus,
  FolderOpen,
  CalendarClock,
  LayoutGrid,
  Search as SearchIcon,
  SquarePen,
} from "lucide-react";
import { useProjectActions } from "@/hooks/use-project-actions";
import { useResolvedKeybinds } from "@/hooks/use-resolved-keybinds";

export function SidebarActionRow() {
  const { state } = useSidebar();
  const { getKeysForAction } = useResolvedKeybinds();
  const newAgentKeys = getKeysForAction("newAgent");
  const paletteKeys = getKeysForAction("commandPalette");
  const setShowCommandPalette = useUIStore((s) => s.setShowCommandPalette);
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
              New agent{newAgentKeys ? ` · ${newAgentKeys}` : ""} · Shift+click for workspace dialog
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

  // Expanded inbox header: a search affordance (opens the command palette)
  // + the accented new-agent button. Add repository moved into the inbox's
  // repo-chip row; Automations / Workspaces moved into the footer app menu.
  return (
    <ShadcnSidebarHeader className="gap-0 p-0">
      {/* Same insets + gap as the repo-chip row below, and every control in
          both rows is h-6 / rounded-[7px], so the two read as equal rows. */}
      <div className="flex items-center gap-1.5 px-2.5 pb-2.5 pt-3">
        <button
          type="button"
          aria-label="Search"
          onClick={() => setShowCommandPalette(true)}
          className="flex h-6 flex-1 cursor-text items-center gap-1.5 rounded-[7px] border border-border/60 bg-foreground/[0.03] px-2 text-muted-foreground/70 transition-colors duration-150 hover:border-border hover:text-muted-foreground"
        >
          <SearchIcon className="h-3 w-3 shrink-0" />
          <span className="flex-1 text-left text-[11.5px]">Search</span>
          {paletteKeys && (
            <kbd className="rounded border border-border/60 px-1 py-0 font-mono text-[9px]">
              {paletteKeys}
            </kbd>
          )}
        </button>
        <Tooltip>
          <TooltipTrigger asChild>
            <button
              type="button"
              aria-label="New agent"
              onClick={handleNewAgent}
              className="flex size-6 shrink-0 items-center justify-center rounded-[7px] border border-accent-ember/35 bg-accent-ember/[0.13] text-accent-ember transition-colors duration-150 hover:bg-accent-ember/20"
            >
              <SquarePen className="size-[13px]" />
            </button>
          </TooltipTrigger>
          <TooltipContent side="bottom" sideOffset={4} className="text-xs">
            New chat in home directory{newAgentKeys ? ` · ${newAgentKeys}` : ""} · Shift+click for workspace dialog
          </TooltipContent>
        </Tooltip>
      </div>
    </ShadcnSidebarHeader>
  );
}
