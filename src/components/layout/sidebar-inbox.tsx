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
  selectSidebarAutoSettleDays,
} from "@/stores/settings-store";
import { formatElapsed } from "@/stores/sidebar-density-store";
import { getWorkspaceStatus } from "@/lib/pane-status";
import { useCoarseClock } from "@/lib/use-coarse-clock";
import { useProjectActions } from "@/hooks/use-project-actions";
import { useProjectAppearance } from "./use-project-appearance";
import { SidebarInboxCard, type InboxRepo } from "./sidebar-inbox-card";
import { activateWorkspace } from "@/tauri/commands";
import { normalizePrState } from "@/components/github/pr-status-icon";
import { useResolvedKeybinds } from "@/hooks/use-resolved-keybinds";
import { parseKeyCombo } from "@/lib/keybind-utils";
import {
  setJumpTargets,
  DEFAULT_JUMP_MODIFIER,
} from "./sidebar-inbox-jump";
import type { WorkspaceSnapshot } from "@/tauri/types";

/** How many leading cards get a jump badge — the digit shortcuts only reach 1-9. */
const MAX_JUMP_HINTS = 9;

/** How long the settle collapse runs before the card actually moves below
 *  the divider. Matches the card wrapper's `duration-200`. */
const SETTLE_ANIM_MS = 200;
/** How long the rise-in ease on a just-settled / just-un-settled row is kept
 *  before the marker clears. */
const ROW_IN_MS = 400;

/** Settled-tail paging: show a short head on first paint, then reveal a larger
 *  page at a time so the settled section can never dominate the sidebar. */
const SETTLED_INITIAL_COUNT = 10;
const SETTLED_PAGE_COUNT = 25;

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
  const autoSettleDays = useSettingsStore(selectSidebarAutoSettleDays);

  const load = useSidebarInboxStore((s) => s.load);
  const loaded = useSidebarInboxStore((s) => s.loaded);
  const settled = useSidebarInboxStore((s) => s.settled);
  const keepActive = useSidebarInboxStore((s) => s.keepActive);
  const activity = useSidebarInboxStore((s) => s.activity);
  const filter = useSidebarInboxStore((s) => s.filter);
  const setFilter = useSidebarInboxStore((s) => s.setFilter);
  const prune = useSidebarInboxStore((s) => s.prune);

  // One coarse (~30s) clock for every elapsed label in the list, and the tick
  // the activity + auto-settle effects re-run on.
  const now = useCoarseClock(true);

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

  // Settled-tail paging window. Reset to the short head whenever the repo
  // filter changes so a newly-scoped list starts collapsed again. Settle /
  // unsettle deliberately don't reset it — the slice just shows fewer rows.
  const [settledVisibleCount, setSettledVisibleCount] = useState(
    SETTLED_INITIAL_COUNT,
  );
  useEffect(() => {
    setSettledVisibleCount(SETTLED_INITIAL_COUNT);
  }, [filter]);
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
    // The settled-row button is an explicit "keep this active" — pin it so the
    // idle/PR auto-settle rules leave it alone until its agent runs again.
    useSidebarInboxStore.getState().unsettle(workspaceId, "user");
    setJustUnsettledId(workspaceId);
    timeoutsRef.current.push(
      window.setTimeout(
        () => setJustUnsettledId((cur) => (cur === workspaceId ? null : cur)),
        ROW_IN_MS,
      ),
    );
  };

  // Un-settle a workspace because its live agent resurfaced it (not a user
  // gesture): clear any keep-active pin via reason "activity", then run the
  // same rise-in animation the manual path uses.
  const resurface = (workspaceId: string) => {
    useSidebarInboxStore.getState().unsettle(workspaceId, "activity");
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
        resurface(entry.id);
      }
    }
  }, [loaded, paneStatuses, settled, allWorkspaces]);

  // Activity observation: keep a client-side "last active" stamp per workspace
  // so the inactivity auto-settle rule has something to measure against (the
  // backend stamps none). We stamp when the agent is doing something (non-null
  // status), when it's the focused workspace, or when we've never recorded it
  // (first-seen baseline — without this a fresh install would mass-settle
  // everything the moment the idle window first elapsed). Re-runs on the
  // coarse tick so a long-running agent keeps refreshing its stamp; the
  // store's 60s write-throttle keeps that cheap. noteActivity also clears any
  // keep-active pin, so a resurfacing agent un-pins itself here.
  useEffect(() => {
    if (!loaded || !paneStatuses) return;
    const noteActivity = useSidebarInboxStore.getState().noteActivity;
    const at = Date.now();
    for (const ws of allWorkspaces) {
      const status = getWorkspaceStatus(ws.surfaces, paneStatuses);
      const isActiveWs = ws.workspace_id === activeWorkspaceId;
      const unseen = activity[ws.workspace_id] === undefined;
      if (status !== null || isActiveWs || unseen) {
        noteActivity(ws.workspace_id, at);
      }
    }
  }, [loaded, paneStatuses, allWorkspaces, activeWorkspaceId, activity, now]);

  // Auto-settle: sweep an active card under the divider on its own once it is
  // safely idle — either its PR has merged/closed, or it has gone untouched
  // past the user's idle window. No leaving animation (this is a background
  // sweep, not a gesture); we do set the justSettled marker so the arriving
  // settled row still eases in.
  //
  // Anti-fight invariants — these three guards make oscillation impossible:
  //   • auto-settle ONLY fires at status null (never live / blocked / review);
  //   • the auto-un-settle safety net above ONLY fires at working/permission;
  //   • a keep-active pin blocks auto-settle until noteActivity (a non-null
  //     status) clears it.
  // So the two effects can never act on the same workspace at the same status,
  // and a user-kept card stays put until its agent genuinely runs again.
  useEffect(() => {
    if (!loaded || !paneStatuses) return;
    const store = useSidebarInboxStore.getState();
    const settledSet = new Set(settled.map((e) => e.id));
    for (const ws of allWorkspaces) {
      const id = ws.workspace_id;
      if (settledSet.has(id)) continue;
      if (keepActive[id]) continue;
      const status = getWorkspaceStatus(ws.surfaces, paneStatuses);
      if (status !== null) continue;
      const prState = normalizePrState(ws.pr_state);
      const prDone = prState === "merged" || prState === "closed";
      const stamp = activity[id];
      const idleSwept =
        autoSettleDays !== null &&
        stamp !== undefined &&
        now - stamp > autoSettleDays * 86_400_000;
      if (prDone || idleSwept) {
        store.settle(id);
        setJustSettledId(id);
        timeoutsRef.current.push(
          window.setTimeout(
            () => setJustSettledId((cur) => (cur === id ? null : cur)),
            ROW_IN_MS,
          ),
        );
      }
    }
  }, [
    loaded,
    paneStatuses,
    allWorkspaces,
    settled,
    keepActive,
    activity,
    autoSettleDays,
    now,
  ]);

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

  // ── Jump-to-card shortcuts ──
  // Publish the visible active-card ids (view order, filter-scoped) so the
  // window-level keyboard handler can resolve "jump to workspace N" without
  // coupling to React. Settled rows are never jump targets. Keyed on a joined
  // string so the effect only re-runs when the visible set actually changes.
  const activeCardIdsKey = activeCards.map((ws) => ws.workspace_id).join(" ");
  useEffect(() => {
    setJumpTargets(activeCardIdsKey ? activeCardIdsKey.split(" ") : []);
  }, [activeCardIdsKey]);
  useEffect(() => () => setJumpTargets([]), []);

  // Which physical modifier reveals the jump badges. Respect the user's actual
  // resolved binding for slot 1 (so a rebind to Ctrl/Alt tracks the right key);
  // fall back to the default modifier. A rebind to a non-Alt/Ctrl chord (e.g.
  // Shift-only) simply shows no held-modifier hints.
  const { keybindMap } = useResolvedKeybinds();
  const jumpModifierKey = useMemo(() => {
    const keys =
      keybindMap.get("workspaceJump1")?.activeKeys ??
      `${DEFAULT_JUMP_MODIFIER}+1`;
    const parsed = parseKeyCombo(keys);
    if (parsed.ctrl) return "Control";
    if (parsed.alt) return "Alt";
    return null;
  }, [keybindMap]);

  // Show the badges only while the modifier is physically held. Clear on keyup,
  // blur, and visibilitychange so the hints can never get stuck open.
  const [jumpHintsVisible, setJumpHintsVisible] = useState(false);
  useEffect(() => {
    if (!jumpModifierKey) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === jumpModifierKey) setJumpHintsVisible(true);
    };
    const onKeyUp = (e: KeyboardEvent) => {
      if (e.key === jumpModifierKey) setJumpHintsVisible(false);
    };
    const clear = () => setJumpHintsVisible(false);
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    window.addEventListener("blur", clear);
    document.addEventListener("visibilitychange", clear);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
      window.removeEventListener("blur", clear);
      document.removeEventListener("visibilitychange", clear);
      setJumpHintsVisible(false);
    };
  }, [jumpModifierKey]);

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

        {activeCards.map((ws, index) => {
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
              jumpHint={
                jumpHintsVisible && index < MAX_JUMP_HINTS ? index + 1 : null
              }
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
            {settledRows.slice(0, settledVisibleCount).map(({ entry, workspace }) => {
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
            {settledRows.length > settledVisibleCount &&
              (() => {
                const hidden = settledRows.length - settledVisibleCount;
                const next = Math.min(SETTLED_PAGE_COUNT, hidden);
                return (
                  <button
                    type="button"
                    data-settled-more
                    aria-label={`Show ${next} more settled workspaces (${hidden} hidden)`}
                    onClick={() =>
                      setSettledVisibleCount((c) => c + SETTLED_PAGE_COUNT)
                    }
                    className="flex h-7 w-full items-center justify-center rounded-lg font-mono text-[10.5px] text-muted-foreground/70 transition-colors duration-150 hover:text-foreground"
                  >
                    {`Show ${next} more (${hidden} hidden)`}
                  </button>
                );
              })()}
          </>
        )}
      </div>
    </div>
  );
}
