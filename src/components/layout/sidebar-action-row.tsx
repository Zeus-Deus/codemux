import { SidebarHeader as ShadcnSidebarHeader, useSidebar } from "@/components/ui/sidebar";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useUIStore } from "@/stores/ui-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { Search as SearchIcon, SquarePen } from "lucide-react";
import { useResolvedKeybinds } from "@/hooks/use-resolved-keybinds";

export function SidebarActionRow() {
  const { state } = useSidebar();
  const { getKeysForAction } = useResolvedKeybinds();
  const newAgentKeys = getKeysForAction("newAgent");
  const paletteKeys = getKeysForAction("commandPalette");
  const setShowCommandPalette = useUIStore((s) => s.setShowCommandPalette);
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

  // Collapsed icon rail header: just the two create/find affordances,
  // centered and each labelled by a right-side tooltip. New agent is a neutral
  // ghost matching the expanded header's pencil; Search opens the command palette.
  // Automations / Workspaces now live in the footer, and Add repository lives
  // in the expanded inbox's repo-chip row — none of them belong here anymore.
  if (state === "collapsed") {
    return (
      <ShadcnSidebarHeader className="gap-0 p-0">
        <div className="flex flex-col items-center gap-1.5 px-1 py-2">
          <Tooltip delayDuration={300}>
            <TooltipTrigger asChild>
              <button
                type="button"
                aria-label="New agent"
                onClick={handleNewAgent}
                className="flex size-7 items-center justify-center rounded-lg border border-border/60 bg-foreground/[0.03] text-muted-foreground transition-colors duration-150 hover:border-border hover:text-foreground"
              >
                <SquarePen className="size-[13px]" />
              </button>
            </TooltipTrigger>
            <TooltipContent side="right" className="text-xs">
              New agent{newAgentKeys ? ` · ${newAgentKeys}` : ""} · Shift+click for workspace dialog
            </TooltipContent>
          </Tooltip>

          <Tooltip delayDuration={300}>
            <TooltipTrigger asChild>
              <button
                type="button"
                aria-label="Search"
                onClick={() => setShowCommandPalette(true)}
                className="flex size-7 items-center justify-center rounded-lg text-muted-foreground transition-colors duration-150 hover:bg-foreground/[0.04] hover:text-foreground"
              >
                <SearchIcon className="size-3.5" />
              </button>
            </TooltipTrigger>
            <TooltipContent side="right" className="text-xs">
              Search{paletteKeys ? ` · ${paletteKeys}` : ""}
            </TooltipContent>
          </Tooltip>

          <div className="my-1 h-px w-[26px] bg-border/60" />
        </div>
      </ShadcnSidebarHeader>
    );
  }

  // Expanded inbox header: a search affordance (opens the command palette)
  // + the neutral-ghost new-agent button. Add repository moved into the inbox's
  // repo-chip row; Automations / Workspaces moved into the footer app menu.
  return (
    <ShadcnSidebarHeader className="gap-0 p-0">
      {/* Same insets + gap as the project-filter row below, and every control
          in both rows is h-8 / rounded-[7px], so the two read as equal rows. */}
      <div className="flex items-center gap-1.5 px-2.5 pb-2.5 pt-3">
        <button
          type="button"
          aria-label="Search"
          onClick={() => setShowCommandPalette(true)}
          className="flex h-8 flex-1 cursor-text items-center gap-2 rounded-[7px] border border-border/60 bg-foreground/[0.03] px-2.5 text-muted-foreground/70 transition-colors duration-150 hover:border-border hover:text-muted-foreground"
        >
          <SearchIcon className="h-3.5 w-3.5 shrink-0" />
          <span className="flex-1 text-left text-xs">Search</span>
          {paletteKeys && (
            <kbd className="rounded border border-border/60 px-1 py-px font-mono text-[10px]">
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
              className="flex size-8 shrink-0 items-center justify-center rounded-[7px] border border-border/60 bg-foreground/[0.03] text-muted-foreground transition-colors duration-150 hover:border-border hover:text-foreground"
            >
              <SquarePen className="size-[15px]" />
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
