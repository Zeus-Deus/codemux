import { useEffect, useMemo } from "react";
import { ProjectAvatar } from "@/components/ui/project-avatar";
import { WorkspaceHoverCard } from "./workspace-hover-card";
import {
  selectActiveWorkspaceId,
  useAppStore,
  useHomeDir,
  useProjectGroupedWorkspaces,
} from "@/stores/app-store";
import { useHosts } from "@/stores/hosts-store";
import { useSidebarInboxStore } from "@/stores/sidebar-inbox-store";
import { compareNewestFirst, isWorkspaceUnread } from "./sidebar-inbox";
import { activateWorkspaceInteraction } from "@/lib/perf/instrumented-activate";
import { getWorkspaceStatus, STATUS_DOT_CLASS } from "@/lib/pane-status";
import { useProjectAppearance } from "./use-project-appearance";
import { cn } from "@/lib/utils";
import type { WorkspaceSnapshot } from "@/tauri/types";

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
    activateWorkspaceInteraction(workspace.workspace_id).catch(console.error);
  };

  // Rail parity with the expanded inbox's background recede: a button whose
  // agent is quietly working, or that is idle and already read, is not asking
  // for anything yet, so it sits back and lets the needs-you / done-review /
  // unread buttons be the bright ones. Hover restores it in full. Unread is
  // the pure stamp predicate the inbox derives from — the rail carries no
  // unread affordance of its own and no manual override, hence `false`; and
  // it has no "Woke" concept at all, so there is nothing to skip for.
  const unread = isWorkspaceUnread(
    workspace.last_active_at,
    workspace.last_visited_at,
    false,
  );
  const receded =
    !isActive &&
    !unread &&
    (status === "working" || status === "monitoring" || status === null);

  // Per-workspace status dot: red (pulse) needs-you > amber working > cyan
  // (steady) monitoring > green review (shared `STATUS_DOT_CLASS`). Idle /
  // null shows nothing.

  // Collapsed to a 28px avatar, the rail shows nothing but a status dot — so
  // the same hover card the expanded inbox uses is the only way to tell two
  // workspaces of one project apart without expanding the sidebar.
  return (
    <WorkspaceHoverCard workspace={workspace} repo={repo} status={status}>
        <button
          type="button"
          data-rail-ws={workspace.workspace_id}
          onClick={handleClick}
          aria-label={workspace.title}
          className={cn(
            "relative flex size-7 items-center justify-center rounded-lg border duration-150",
            "transition-[color,background-color,border-color,opacity]",
            isActive
              ? "border-border bg-foreground/[0.09]"
              : "border-transparent hover:bg-foreground/[0.04]",
            // The status dot inherits the dim along with the avatar, which is
            // the point: a quietly-working button should read as quieter as a
            // whole, not as a dim avatar wearing a full-strength badge.
            receded && "opacity-70 hover:opacity-100",
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
                STATUS_DOT_CLASS[status],
              )}
            />
          )}
        </button>
    </WorkspaceHoverCard>
  );
}

/**
 * Collapsed (icon-rail) rendering of the workspace inbox: a centered vertical
 * strip with one avatar button per ACTIVE workspace — neither settled nor
 * snoozed — in the same order as the expanded inbox. The repo filter never
 * applies here (the rail always shows every active workspace), and each
 * button's corner dot mirrors that workspace's own agent status so activity
 * stays visible while collapsed.
 */
export function SidebarRailWorkspaces() {
  const appState = useAppStore((s) => s.appState);
  const allWorkspaces = useMemo(
    () => appState?.workspaces ?? [],
    [appState?.workspaces],
  );
  // Pending-aware so the highlight moves in the click's own task, before the
  // backend snapshot lands (docs/plans/gui-responsiveness.md, Phase 1).
  const activeWorkspaceId = useAppStore(selectActiveWorkspaceId) ?? "";
  const homeDir = useHomeDir();
  const hosts = useHosts();
  const projectGroups = useProjectGroupedWorkspaces(
    allWorkspaces,
    homeDir,
    hosts,
  );

  const load = useSidebarInboxStore((s) => s.load);
  const loaded = useSidebarInboxStore((s) => s.loaded);
  const settled = useSidebarInboxStore((s) => s.settled);
  const snoozed = useSidebarInboxStore((s) => s.snoozed);
  const prune = useSidebarInboxStore((s) => s.prune);

  useEffect(() => {
    void load();
  }, [load]);

  // Same prune the expanded inbox runs: drop persisted entries whose workspace
  // vanished (archived / deleted). Mirrored here so a session spent entirely
  // in the collapsed rail still trims the blob.
  useEffect(() => {
    if (!loaded || !appState) return;
    prune(new Set(allWorkspaces.map((w) => w.workspace_id)));
  }, [loaded, appState, allWorkspaces, prune]);

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

  // Both parking lifecycles hide a workspace here, exactly as they do in the
  // expanded inbox. Excluding only `settled` would let a snoozed workspace
  // keep its rail button — the deferred work the user just pushed out of
  // sight would still be sitting there, which is the whole gesture undone.
  const parkedIds = useMemo(
    () => new Set([...settled.map((e) => e.id), ...snoozed.map((e) => e.id)]),
    [settled, snoozed],
  );

  // …with the inbox's one exception: the workspace the user is *in* never
  // hides, whichever shelf it is parked on. The expanded inbox force-renders
  // its row; here the button's selection fill is the only "you are here" the
  // collapsed sidebar has, and dropping it would leave the rail claiming the
  // user is nowhere.
  //
  // Same newest-first, status-blind order as the expanded inbox (the shared
  // `compareNewestFirst`), so collapsing the sidebar never re-shuffles the
  // workspaces the user just memorized positions for.
  const railWorkspaces = allWorkspaces
    .map((ws, storedIndex) => ({ ws, storedIndex }))
    .filter(
      ({ ws }) =>
        !parkedIds.has(ws.workspace_id) ||
        ws.workspace_id === activeWorkspaceId,
    )
    .sort(compareNewestFirst)
    .map(({ ws }) => ws);

  return (
    <div className="flex flex-1 min-h-0 flex-col items-center gap-1.5 overflow-y-auto py-1 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
      {railWorkspaces.map((ws) => {
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
