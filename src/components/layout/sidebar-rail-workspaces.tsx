import { startTransition, useEffect, useMemo } from "react";
import { ProjectAvatar } from "@/components/ui/project-avatar";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import {
  useAppStore,
  useHomeDir,
  useProjectGroupedWorkspaces,
} from "@/stores/app-store";
import { useHosts } from "@/stores/hosts-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useSidebarInboxStore } from "@/stores/sidebar-inbox-store";
import { activateWorkspace } from "@/tauri/commands";
import { getWorkspaceStatus } from "@/lib/pane-status";
import { useProjectAppearance } from "./use-project-appearance";
import { cn } from "@/lib/utils";
import type { ActivePaneStatus, WorkspaceSnapshot } from "@/tauri/types";

interface RailItemRepo {
  name: string;
  path: string;
}

/** One active workspace as a 28px avatar button in the collapsed rail. Owns
 *  its own appearance load (per-item hook) and a live status subscription so
 *  its corner dot stays current while the sidebar is collapsed. Clicking
 *  activates the workspace without expanding the sidebar. */
function RailWorkspaceItem({
  workspace,
  repo,
  isActive,
}: {
  workspace: WorkspaceSnapshot;
  repo: RailItemRepo;
  isActive: boolean;
}) {
  const { customColor, imageUrl, imageVersion } = useProjectAppearance(
    repo.path,
  );
  const status = useAppStore((s) =>
    s.appState
      ? getWorkspaceStatus(workspace.surfaces, s.appState.pane_statuses)
      : null,
  );

  const handleClick = () => {
    useChatDraftStore.getState().setActiveDraft(null);
    startTransition(() => {
      activateWorkspace(workspace.workspace_id).catch(console.error);
    });
  };

  // Per-workspace status dot: red (pulse) needs-you > amber working >
  // green review. Idle / null shows nothing.
  const dotClass: Record<ActivePaneStatus, string> = {
    permission: "bg-status-attention animate-pulse",
    working: "bg-status-working",
    review: "bg-status-open",
  };

  return (
    <Tooltip delayDuration={300}>
      <TooltipTrigger asChild>
        <button
          type="button"
          data-rail-ws={workspace.workspace_id}
          onClick={handleClick}
          aria-label={workspace.title}
          className={cn(
            "relative flex size-7 items-center justify-center rounded-lg border transition-colors duration-150",
            isActive
              ? "border-border bg-foreground/[0.09]"
              : "border-transparent hover:bg-foreground/[0.04]",
          )}
        >
          <ProjectAvatar
            name={repo.name}
            color={customColor}
            imageUrl={imageUrl}
            cacheBust={imageVersion}
            size="md"
            shape="square"
          />
          {status && (
            <span
              className={cn(
                "absolute right-0.5 top-0.5 size-[7px] rounded-full border-[1.5px] border-sidebar",
                dotClass[status],
              )}
            />
          )}
        </button>
      </TooltipTrigger>
      <TooltipContent side="right" className="text-xs">
        {workspace.title}
      </TooltipContent>
    </Tooltip>
  );
}

/**
 * Collapsed (icon-rail) rendering of the workspace inbox: a centered vertical
 * strip with one avatar button per ACTIVE (unsettled) workspace, in the same
 * order as the expanded inbox. The repo filter never applies here — the rail
 * always shows every active workspace — and each button's corner dot mirrors
 * that workspace's own agent status so activity stays visible while collapsed.
 */
export function SidebarRailWorkspaces() {
  const appState = useAppStore((s) => s.appState);
  const allWorkspaces = useMemo(
    () => appState?.workspaces ?? [],
    [appState?.workspaces],
  );
  const activeWorkspaceId = appState?.active_workspace_id ?? "";
  const homeDir = useHomeDir();
  const hosts = useHosts();
  const projectGroups = useProjectGroupedWorkspaces(
    allWorkspaces,
    homeDir,
    hosts,
  );

  const load = useSidebarInboxStore((s) => s.load);
  const settled = useSidebarInboxStore((s) => s.settled);

  useEffect(() => {
    void load();
  }, [load]);

  // workspace_id → project identity, from the same grouping pipeline the
  // expanded inbox uses (dedup'd names, Home labeling, host suffixes).
  const repoByWorkspace = useMemo(() => {
    const map = new Map<string, RailItemRepo>();
    for (const group of projectGroups) {
      for (const ws of group.workspaces) {
        map.set(ws.workspace_id, {
          name: group.projectName,
          path: group.projectPath,
        });
      }
    }
    return map;
  }, [projectGroups]);

  const settledIds = useMemo(
    () => new Set(settled.map((e) => e.id)),
    [settled],
  );

  const activeWorkspaces = allWorkspaces.filter(
    (ws) => !settledIds.has(ws.workspace_id),
  );

  return (
    <div className="flex flex-1 min-h-0 flex-col items-center gap-1.5 overflow-y-auto py-1 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
      {activeWorkspaces.map((ws) => {
        const repo = repoByWorkspace.get(ws.workspace_id);
        if (!repo) return null;
        return (
          <RailWorkspaceItem
            key={ws.workspace_id}
            workspace={ws}
            repo={repo}
            isActive={ws.workspace_id === activeWorkspaceId}
          />
        );
      })}
    </div>
  );
}
