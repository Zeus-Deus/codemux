import { SidebarHeader as ShadcnSidebarHeader, useSidebar } from "@/components/ui/sidebar";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { Toggle } from "@/components/ui/toggle";
import { startNewHomeAgent } from "@/lib/agent-chat/new-home-agent";
import { useUIStore } from "@/stores/ui-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { ListTree, Search as SearchIcon, SquarePen } from "lucide-react";
import { useResolvedKeybinds } from "@/hooks/use-resolved-keybinds";
import { useTitlebarOverlay } from "@/hooks/use-gui-chrome";
import { cn } from "@/lib/utils";
import { useSidebarViewStore } from "./sidebar-view-store";
import { PluginSidebarLayoutMenu } from "@/components/plugins/PluginTreeSidebar";

/** The "New agent" gesture shared by the sidebar header and its empty state:
 *  a home-directory chat draft when agent chat is on, otherwise (or with
 *  Shift held) the new-workspace dialog. */
export function useNewAgentAction() {
  const setShowNewWorkspaceDialog = useUIStore((s) => s.setShowNewWorkspaceDialog);
  const enableAgentChat = useFeatureFlags((s) => s.enableAgentChat);
  const enableLazyWorkspaceCreation = useFeatureFlags(
    (s) => s.enableLazyWorkspaceCreation,
  );

  const handleNewAgent = (e: React.MouseEvent) => {
    if (e.shiftKey || !enableAgentChat) {
      setShowNewWorkspaceDialog(true);
      return;
    }
    if (enableLazyWorkspaceCreation) {
      startNewHomeAgent();
      return;
    }
    setShowNewWorkspaceDialog(true);
  };
  return handleNewAgent;
}

export function SidebarActionRow() {
  const { state } = useSidebar();
  // Only the floating titlebar overlays this header. With legacy chrome the
  // in-flow `h-9` bar already sits above the sidebar, so the extra clearance
  // would just be dead padding above the header row.
  const titlebarOverlay = useTitlebarOverlay();
  const { getKeysForAction } = useResolvedKeybinds();
  const newAgentKeys = getKeysForAction("newAgent");
  const paletteKeys = getKeysForAction("commandPalette");
  const setShowCommandPalette = useUIStore((s) => s.setShowCommandPalette);
  const handleNewAgent = useNewAgentAction();

  // Collapsed icon rail header: just the two create/find affordances,
  // centered and each labelled by a right-side tooltip. Both are the same
  // quiet ghost as the expanded header's icons; Search opens the command palette.
  // Automations / Workspaces now live in the footer, and Open / New project
  // live at the bottom of the expanded inbox's project menu — none of them belong here anymore.
  if (state === "collapsed") {
    return (
      <ShadcnSidebarHeader className="gap-0 p-0">
        <div
          data-testid="sidebar-action-row-collapsed"
          className={cn(
            "flex flex-col items-center gap-1.5 px-1 pb-2",
            titlebarOverlay ? "pt-11" : "pt-2",
          )}
        >
          <Tooltip delayDuration={300}>
            <TooltipTrigger asChild>
              <button
                type="button"
                aria-label="New agent"
                onClick={handleNewAgent}
                className="flex size-7 items-center justify-center rounded-lg text-muted-foreground transition-colors duration-150 hover:bg-surface-2 hover:text-foreground"
              >
                <SquarePen className="size-[13px]" />
              </button>
            </TooltipTrigger>
            <TooltipContent side="right">
              New agent{newAgentKeys ? ` · ${newAgentKeys}` : ""} · Shift+click for workspace dialog
            </TooltipContent>
          </Tooltip>

          <Tooltip delayDuration={300}>
            <TooltipTrigger asChild>
              <button
                type="button"
                aria-label="Search"
                onClick={() => setShowCommandPalette(true)}
                className="flex size-7 items-center justify-center rounded-lg text-muted-foreground transition-colors duration-150 hover:bg-surface-2 hover:text-foreground"
              >
                <SearchIcon className="size-3.5" />
              </button>
            </TooltipTrigger>
            <TooltipContent side="right">
              Search{paletteKeys ? ` · ${paletteKeys}` : ""}
            </TooltipContent>
          </Tooltip>

          <div className="my-1 h-px w-[26px] bg-border/60" />
        </div>
      </ShadcnSidebarHeader>
    );
  }

  // Expanded: the header itself is only the titlebar clearance. Search and
  // New agent render inside the inbox's project-filter row
  // (`SidebarHeaderActions`) so the whole header is a single row.
  return (
    <ShadcnSidebarHeader className="gap-0 p-0">
      <div
        data-testid="sidebar-action-row-expanded"
        className={titlebarOverlay ? "pt-11" : "pt-3"}
      />
    </ShadcnSidebarHeader>
  );
}

/** Projects is an optional view: unpressing the toggle returns to Activity. */
export function SidebarViewSwitch() {
  const view = useSidebarViewStore((s) => s.view);
  const setView = useSidebarViewStore((s) => s.setView);
  const { getKeysForAction } = useResolvedKeybinds();
  const toggleKeys = getKeysForAction("toggleSidebarView");

  return (
    <Tooltip>
      <Toggle
        asChild
        pressed={view === "projects"}
        onPressedChange={(pressed) => setView(pressed ? "projects" : "activity")}
        aria-label="Projects view"
        data-sidebar-view-switch
        data-sidebar-view="projects"
        className="size-8 shrink-0 rounded-md p-0"
      >
        <TooltipTrigger>
          <ListTree className="size-[15px]" />
        </TooltipTrigger>
      </Toggle>
      <TooltipContent side="bottom">
        Projects view{toggleKeys ? ` · ${toggleKeys}` : ""}
      </TooltipContent>
    </Tooltip>
  );
}

/** View switch + Search + New agent, sized to sit at the end of the expanded
 *  sidebar's project-filter row. Search is icon-only here: the palette
 *  shortcut moves into its tooltip so the project name keeps the row's width.
 *  No outlines: the switch is grouped by its fill and the two actions are
 *  plain ghosts, so the row reads as one calm toolbar. */
export function SidebarHeaderActions() {
  const { getKeysForAction } = useResolvedKeybinds();
  const newAgentKeys = getKeysForAction("newAgent");
  const paletteKeys = getKeysForAction("commandPalette");
  const setShowCommandPalette = useUIStore((s) => s.setShowCommandPalette);
  const handleNewAgent = useNewAgentAction();

  return (
    <>
      <SidebarViewSwitch />
      <PluginSidebarLayoutMenu />
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            type="button"
            aria-label="Search"
            onClick={() => setShowCommandPalette(true)}
            className="flex size-8 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors duration-150 hover:bg-surface-2 hover:text-foreground"
          >
            <SearchIcon className="size-[15px]" />
          </button>
        </TooltipTrigger>
        <TooltipContent side="bottom">
          Search{paletteKeys ? ` · ${paletteKeys}` : ""}
        </TooltipContent>
      </Tooltip>
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            type="button"
            aria-label="New agent"
            onClick={handleNewAgent}
            className="flex size-8 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors duration-150 hover:bg-surface-2 hover:text-foreground"
          >
            <SquarePen className="size-[15px]" />
          </button>
        </TooltipTrigger>
        <TooltipContent side="bottom">
          New chat in home directory{newAgentKeys ? ` · ${newAgentKeys}` : ""} · Shift+click for workspace dialog
        </TooltipContent>
      </Tooltip>
    </>
  );
}
