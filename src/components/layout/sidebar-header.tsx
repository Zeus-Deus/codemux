import { SidebarHeader as ShadcnSidebarHeader, SidebarSeparator } from "@/components/ui/sidebar";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useUIStore } from "@/stores/ui-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { Plus } from "lucide-react";

export function SidebarHeader() {
  const setShowDialog = useUIStore((s) => s.setShowNewWorkspaceDialog);
  const enableAgentChat = useFeatureFlags((s) => s.enableAgentChat);
  const enableLazyWorkspaceCreation = useFeatureFlags(
    (s) => s.enableLazyWorkspaceCreation,
  );

  const handlePlusClick = (e: React.MouseEvent) => {
    // Shift+click or agent-chat flag off → open NewWorkspaceDialog.
    if (e.shiftKey || !enableAgentChat) {
      setShowDialog(true);
      return;
    }

    // Lazy-creation path: open (or reuse) the single-slot Home draft
    // without materialising a workspace. The draft surface renders via
    // WorkspaceMain's lazy branch.
    if (enableLazyWorkspaceCreation) {
      const store = useChatDraftStore.getState();
      const draft = store.getOrCreateHomeDraft();
      store.setActiveDraft(draft.draftId);
      return;
    }

    // Legacy path (agent_chat on, lazy off): the old `openHomeChat`
    // helper has been removed along with the Home singleton. There's
    // no sensible eager "create a Home workspace" path anymore, so
    // plain-click falls back to the same dialog as Shift+click.
    setShowDialog(true);
  };

  return (
    <ShadcnSidebarHeader className="gap-0 p-0">
      {/* Home-chat entry row. Mirrors per-project "+" buttons but
          always opens a Home draft rooted at $HOME regardless of
          active project context. Shift+click still falls through
          to the New Workspace dialog (legacy / CLI path). */}
      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            variant="ghost"
            aria-label="New agent"
            className="w-full justify-start pl-3 pr-2 py-3 h-auto text-muted-foreground hover:bg-sidebar-accent hover:text-sidebar-accent-foreground"
            onClick={handlePlusClick}
          >
            <div className="size-6 flex items-center justify-center shrink-0 mr-2.5">
              <Plus className="h-3.5 w-3.5" />
            </div>
            <span>New Agent</span>
          </Button>
        </TooltipTrigger>
        <TooltipContent side="bottom" sideOffset={4} className="text-xs">
          New chat in home directory · Shift+click for workspace dialog
        </TooltipContent>
      </Tooltip>
      <SidebarSeparator />
    </ShadcnSidebarHeader>
  );
}
