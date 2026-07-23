import { startTransition, useEffect, useMemo, useRef, useState } from "react";
import { AlertCircle, FolderOpen, FolderPlus, GitMerge, Loader2, Undo2 } from "lucide-react";
import { cn } from "@/lib/utils";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ProjectAvatar } from "@/components/ui/project-avatar";
import {
  useAppStore,
  useHomeDir,
  useProjectGroupedWorkspaces,
} from "@/stores/app-store";
import { useHosts } from "@/stores/hosts-store";
import { useUIStore } from "@/stores/ui-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useSidebarInboxStore } from "@/stores/sidebar-inbox-store";
import {
  useSettingsStore,
  selectSidebarShowGitStats,
} from "@/stores/settings-store";
import { formatElapsed } from "@/stores/sidebar-density-store";
import { getWorkspaceStatus } from "@/lib/pane-status";
import { useCoarseClock } from "@/lib/use-coarse-clock";
import { useProjectActions } from "@/hooks/use-project-actions";
import { useProjectAppearance } from "./use-project-appearance";
import { SidebarInboxCard, type InboxRepo } from "./sidebar-inbox-card";
import { activateWorkspace } from "@/tauri/commands";
import { normalizePrState } from "@/components/github/pr-status-icon";
import type { WorkspaceSnapshot } from "@/tauri/types";

/** How long the settle collapse runs before the card actually moves below
 *  the divider. Matches the card wrapper's `duration-200`. */
const SETTLE_ANIM_MS = 200;
/** How long the rise-in ease on a just-settled / just-un-settled row is kept
 *  before the marker clears. */
const ROW_IN_MS = 400;

interface RepoChipProps {
  label: string;
  projectPath: string | null;
  active: boolean;
  onClick: () => void;
}

function RepoChip({ label, projectPath, active, onClick }: RepoChipProps) {
  // Hooks must run unconditionally; the "All" chip just passes a blank path
  // and renders no avatar.
  const appearance = useProjectAppearance(projectPath ?? "");
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active}
      aria-label={
        projectPath !== null ? `Filter by ${label}` : "Show all repositories"
      }
      className={cn(
        "flex h-6 shrink-0 items-center gap-1.5 rounded-[7px] border px-2 text-[11px] font-semibold",
        "transition-colors duration-150",
        active
          ? "border-accent-ember/40 bg-accent-ember/10 text-foreground"
          : "border-border/60 bg-transparent text-muted-foreground hover:border-border hover:text-foreground",
      )}
    >
      {projectPath !== null && (
        <ProjectAvatar
          name={label}
          color={appearance.customColor}
          imageUrl={appearance.imageUrl}
          cacheBust={appearance.imageVersion}
          size="sm"
          shape="square"
          className="font-bold"
        />
      )}
      {label}
    </button>
  );
}

interface SettledRowProps {
  workspace: WorkspaceSnapshot;
  repo: InboxRepo;
  isActive: boolean;
  /** Elapsed-since-settle label ("2h"), or null when unknown. */
  time: string | null;
  justSettled: boolean;
  onUnsettle: (workspaceId: string) => void;
}

function SettledRow({
  workspace,
  repo,
  isActive,
  time,
  justSettled,
  onUnsettle,
}: SettledRowProps) {
  const appearance = useProjectAppearance(repo.path);
  const merged = normalizePrState(workspace.pr_state) === "merged";

  const handleActivate = () => {
    useChatDraftStore.getState().setActiveDraft(null);
    startTransition(() => {
      activateWorkspace(workspace.workspace_id).catch(console.error);
    });
  };

  return (
    <div
      role="button"
      tabIndex={0}
      data-settled-row={workspace.workspace_id}
      onClick={handleActivate}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") handleActivate();
      }}
      className={cn(
        "group/settled flex h-[30px] cursor-pointer items-center gap-2 rounded-lg px-2",
        "outline-none transition-colors duration-150 hover:bg-foreground/[0.045] focus-visible:bg-foreground/[0.045]",
        isActive && "bg-foreground/[0.06]",
        justSettled && "rise-in",
      )}
    >
      <ProjectAvatar
        name={repo.name}
        color={appearance.customColor}
        imageUrl={appearance.imageUrl}
        cacheBust={appearance.imageVersion}
        size="sm"
        shape="square"
        className="font-bold opacity-80"
      />
      {merged && (
        <GitMerge
          aria-label="PR merged"
          className="h-3 w-3 shrink-0 text-accent-violet"
        />
      )}
      <span className="min-w-0 flex-1 truncate text-xs font-medium text-muted-foreground">
        {workspace.title}
      </span>
      {time && (
        <span className="shrink-0 text-[10.5px] text-muted-foreground/70 group-hover/settled:hidden group-focus-within/settled:hidden">
          {time}
        </span>
      )}
      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation();
          onUnsettle(workspace.workspace_id);
        }}
        aria-label={`Un-settle "${workspace.title}"`}
        className={cn(
          "hidden h-[19px] shrink-0 items-center gap-1 rounded-md border border-border bg-muted px-[7px]",
          "text-[10px] font-semibold text-muted-foreground transition-colors duration-150",
          "hover:border-muted-foreground/50 hover:text-foreground",
          "group-hover/settled:inline-flex group-focus-within/settled:inline-flex",
        )}
      >
        <Undo2 className="h-2.5 w-2.5" />
        Un-settle
      </button>
    </div>
  );
}

/** The flat workspace inbox that replaces the nested project tree in the
 *  expanded sidebar: a repo filter-chip row, one multi-line card per active
 *  workspace, and a "Settled" section of one-line rows the user has swept
 *  aside. Settling is visual only — nothing is archived or deleted. */
export function SidebarInbox() {
  const appState = useAppStore((s) => s.appState);
  const allWorkspaces = useMemo(
    () => appState?.workspaces ?? [],
    [appState?.workspaces],
  );
  const paneStatuses = appState?.pane_statuses;
  const activeWorkspaceId = appState?.active_workspace_id ?? "";
  const homeDir = useHomeDir();
  const hosts = useHosts();
  const projectGroups = useProjectGroupedWorkspaces(allWorkspaces, homeDir, hosts);
  const pendingWorkspaces = useUIStore((s) => s.pendingWorkspaces);
  const setShowNewProjectScreen = useUIStore((s) => s.setShowNewProjectScreen);
  const { openProject } = useProjectActions();

  const showGitStats = useSettingsStore(selectSidebarShowGitStats);

  const load = useSidebarInboxStore((s) => s.load);
  const loaded = useSidebarInboxStore((s) => s.loaded);
  const settled = useSidebarInboxStore((s) => s.settled);
  const filter = useSidebarInboxStore((s) => s.filter);
  const setFilter = useSidebarInboxStore((s) => s.setFilter);
  const prune = useSidebarInboxStore((s) => s.prune);

  useEffect(() => {
    void load();
  }, [load]);

  // Drop settled entries whose workspace vanished (archived / deleted).
  useEffect(() => {
    if (!loaded || !appState) return;
    prune(new Set(allWorkspaces.map((w) => w.workspace_id)));
  }, [loaded, appState, allWorkspaces, prune]);

  // Settle motion: collapse the card (~200ms), then flip the persisted flag
  // so it re-renders as a one-line row under the divider (which eases in).
  const [leavingId, setLeavingId] = useState<string | null>(null);
  const [justSettledId, setJustSettledId] = useState<string | null>(null);
  const [justUnsettledId, setJustUnsettledId] = useState<string | null>(null);
  const timeoutsRef = useRef<number[]>([]);
  useEffect(
    () => () => {
      timeoutsRef.current.forEach((t) => window.clearTimeout(t));
    },
    [],
  );

  const handleSettle = (workspaceId: string) => {
    setLeavingId(workspaceId);
    timeoutsRef.current.push(
      window.setTimeout(() => {
        useSidebarInboxStore.getState().settle(workspaceId);
        setLeavingId(null);
        setJustSettledId(workspaceId);
        timeoutsRef.current.push(
          window.setTimeout(
            () =>
              setJustSettledId((cur) => (cur === workspaceId ? null : cur)),
            ROW_IN_MS,
          ),
        );
      }, SETTLE_ANIM_MS),
    );
  };

  const handleUnsettle = (workspaceId: string) => {
    useSidebarInboxStore.getState().unsettle(workspaceId);
    setJustUnsettledId(workspaceId);
    timeoutsRef.current.push(
      window.setTimeout(
        () => setJustUnsettledId((cur) => (cur === workspaceId ? null : cur)),
        ROW_IN_MS,
      ),
    );
  };

  // Settle safety net: a settled workspace whose agent goes live ("working")
  // or blocked ("permission") resurfaces into the active list automatically,
  // so live or blocked work can never stay buried under the divider. Finished
  // ("review") and idle settled rows stay put — only fresh activity resurfaces
  // them. Unsettle removes the entry from `settled`, so re-runs converge; we
  // iterate a snapshot and skip ids that aren't currently settled.
  useEffect(() => {
    if (!loaded || !paneStatuses) return;
    for (const entry of settled) {
      const ws = allWorkspaces.find((w) => w.workspace_id === entry.id);
      if (!ws) continue;
      const status = getWorkspaceStatus(ws.surfaces, paneStatuses);
      if (status === "working" || status === "permission") {
        handleUnsettle(entry.id);
      }
    }
  }, [loaded, paneStatuses, settled, allWorkspaces]);

  // workspace_id → repo (project) identity, from the same grouping pipeline
  // the rest of the app uses (dedup'd names, Home labeling, host suffixes).
  const repoByWorkspace = useMemo(() => {
    const map = new Map<string, InboxRepo>();
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

  const matchesFilter = (ws: WorkspaceSnapshot) =>
    !filter || repoByWorkspace.get(ws.workspace_id)?.path === filter;

  // Active cards keep the user's stored workspace order; settled rows keep
  // newest-settled-first. The repo chips filter both lists.
  const activeCards = allWorkspaces.filter(
    (ws) => !settledIds.has(ws.workspace_id) && matchesFilter(ws),
  );
  const settledRows = settled
    .map((entry) => ({
      entry,
      workspace: allWorkspaces.find((w) => w.workspace_id === entry.id),
    }))
    .filter(
      (r): r is { entry: (typeof settled)[number]; workspace: WorkspaceSnapshot } =>
        r.workspace !== undefined && matchesFilter(r.workspace),
    );

  const filteredPending = filter
    ? pendingWorkspaces.filter((pw) => pw.projectPath === filter)
    : pendingWorkspaces;

  // One coarse (~30s) clock for every elapsed label in the list.
  const now = useCoarseClock(true);

  // Vertical wheel → horizontal scroll on the chip strip. The strip is the
  // only horizontal scroller in the sidebar and most mice have no tilt
  // wheel, so plain wheel motion should move it. Native non-passive
  // listener (React's onWheel can't preventDefault) so the same gesture
  // doesn't also scroll the card list underneath.
  const chipStripRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    const el = chipStripRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (el.scrollWidth <= el.clientWidth) return; // nothing to scroll
      // A genuine horizontal wheel (tilt / trackpad swipe) already works —
      // only translate when the motion is predominantly vertical.
      if (Math.abs(e.deltaY) <= Math.abs(e.deltaX)) return;
      el.scrollLeft += e.deltaY;
      e.preventDefault();
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  const filterName = filter
    ? projectGroups.find((g) => g.projectPath === filter)?.projectName ?? null
    : null;

  return (
    <div className="flex flex-col">
      {/* Repo filter chips — sticky so the filter stays reachable while the
          card list scrolls beneath it. */}
      <div className="sticky top-0 z-10 flex items-center gap-1.5 bg-sidebar px-2.5 pb-2.5 pt-0.5 min-w-0">
        <div
          ref={chipStripRef}
          data-chip-strip
          className="flex-1 min-w-0 flex items-center gap-1.5 overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
        >
          <RepoChip
            label="All"
            projectPath={null}
            active={filter === null}
            onClick={() => setFilter(null)}
          />
          {projectGroups.map((group) => (
            <RepoChip
              key={group.projectPath}
              label={group.projectName}
              projectPath={group.projectPath}
              active={filter === group.projectPath}
              onClick={() => setFilter(group.projectPath)}
            />
          ))}
        </div>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              aria-label="Add repository"
              className="flex size-6 shrink-0 items-center justify-center rounded-[7px] border border-dashed border-border text-[13px] leading-none text-muted-foreground transition-colors duration-150 hover:border-muted-foreground/60 hover:text-foreground"
            >
              +
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent side="bottom" align="start">
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

      <div className="px-2.5 pb-2.5">
        {activeCards.length === 0 && filteredPending.length === 0 && (
          <div className="px-2 py-6 text-center text-xs text-muted-foreground">
            Nothing active
            {filterName && (
              <>
                {" in "}
                <span className="font-mono">{filterName}</span>
              </>
            )}
          </div>
        )}

        {activeCards.map((ws) => {
          const repo = repoByWorkspace.get(ws.workspace_id);
          if (!repo) return null;
          return (
            <SidebarInboxCard
              key={ws.workspace_id}
              workspace={ws}
              repo={repo}
              isActive={ws.workspace_id === activeWorkspaceId}
              status={
                paneStatuses
                  ? getWorkspaceStatus(ws.surfaces, paneStatuses)
                  : null
              }
              showGitStats={showGitStats}
              now={now}
              leaving={leavingId === ws.workspace_id}
              justUnsettled={justUnsettledId === ws.workspace_id}
              onSettle={handleSettle}
            />
          );
        })}

        {filteredPending.map((pw) => (
          <div
            key={pw.id}
            className={cn(
              "flex items-center gap-2 px-2 py-2 text-sm",
              pw.status === "failed" ? "opacity-60" : "animate-pulse opacity-70",
            )}
          >
            {pw.status === "creating" ? (
              <Loader2 className="h-3.5 w-3.5 shrink-0 animate-spin text-muted-foreground" />
            ) : (
              <AlertCircle className="h-3.5 w-3.5 shrink-0 text-destructive" />
            )}
            <span className="truncate text-xs text-muted-foreground">
              {pw.status === "failed" ? pw.errorMessage || "Failed" : pw.name}
            </span>
          </div>
        ))}

        {settledRows.length > 0 && (
          <>
            <div className="flex items-center gap-2 px-1 pb-1.5 pt-3">
              <span className="font-mono text-[9.5px] uppercase tracking-[0.13em] text-muted-foreground/70">
                Settled
              </span>
              <span className="h-px flex-1 bg-border/60" />
            </div>
            {settledRows.map(({ entry, workspace }) => {
              const repo = repoByWorkspace.get(workspace.workspace_id);
              if (!repo) return null;
              return (
                <SettledRow
                  key={workspace.workspace_id}
                  workspace={workspace}
                  repo={repo}
                  isActive={workspace.workspace_id === activeWorkspaceId}
                  time={formatElapsed(now - entry.at)}
                  justSettled={justSettledId === workspace.workspace_id}
                  onUnsettle={handleUnsettle}
                />
              );
            })}
          </>
        )}
      </div>
    </div>
  );
}
