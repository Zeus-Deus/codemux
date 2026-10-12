import { SidebarHeader as ShadcnSidebarHeader, useSidebar } from "@/components/ui/sidebar";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { startNewHomeAgent } from "@/lib/agent-chat/new-home-agent";
import { useUIStore } from "@/stores/ui-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { Search as SearchIcon, SquarePen } from "lucide-react";
import { useResolvedKeybinds } from "@/hooks/use-resolved-keybinds";
import { useTitlebarOverlay } from "@/hooks/use-gui-chrome";
import { cn } from "@/lib/utils";

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
  // centered and each labelled by a right-side tooltip. New agent is a neutral
  // ghost matching the expanded header's pencil; Search opens the command palette.
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
                className="flex size-7 items-center justify-center rounded-lg border border-border/60 bg-surface-1 text-muted-foreground transition-colors duration-150 hover:border-border hover:text-foreground"
              >
                <SquarePen className="size-[13px]" />
              </button>
            </TooltipTrigger>
            <TooltipContent side="right" className="text-label">
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
            <TooltipContent side="right" className="text-label">
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

/** Search + New agent, sized to sit at the end of the expanded inbox's
 *  project-filter row. Search is icon-only here: the palette shortcut moves
 *  into its tooltip so the project name keeps the row's width. */
export function SidebarHeaderActions() {
  const { getKeysForAction } = useResolvedKeybinds();
  const newAgentKeys = getKeysForAction("newAgent");
  const paletteKeys = getKeysForAction("commandPalette");
  const setShowCommandPalette = useUIStore((s) => s.setShowCommandPalette);
  const handleNewAgent = useNewAgentAction();

  return (
    <>
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
        <TooltipContent side="bottom" sideOffset={4} className="text-label">
          Search{paletteKeys ? ` · ${paletteKeys}` : ""}
        </TooltipContent>
      </Tooltip>
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            type="button"
            aria-label="New agent"
            onClick={handleNewAgent}
            className="flex size-8 shrink-0 items-center justify-center rounded-md border border-border/60 bg-surface-1 text-muted-foreground transition-colors duration-150 hover:border-border hover:text-foreground"
          >
            <SquarePen className="size-[15px]" />
          </button>
        </TooltipTrigger>
        <TooltipContent side="bottom" sideOffset={4} className="text-label">
          New chat in home directory{newAgentKeys ? ` · ${newAgentKeys}` : ""} · Shift+click for workspace dialog
        </TooltipContent>
      </Tooltip>
    </>
  );
}
