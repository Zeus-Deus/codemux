import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  AlarmClock,
  Check,
  ChevronRight,
  Circle,
  CircleDotDashed,
  Plus,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { ProjectAvatar } from "@/components/ui/project-avatar";
import { Eyebrow } from "@/components/ui/eyebrow";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import {
  SidebarGroup,
  SidebarGroupAction,
  SidebarGroupLabel,
  SidebarMenu,
  SidebarMenuAction,
  SidebarMenuItem,
  SidebarMenuSub,
  SidebarMenuSubItem,
} from "@/components/ui/sidebar";
import {
  selectActiveWorkspaceId,
  useAppStore,
  useHomeDir,
  useProjectGroupedWorkspaces,
} from "@/stores/app-store";
import { useHosts } from "@/stores/hosts-store";
import { useUIStore } from "@/stores/ui-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import {
  resolveSettledTimestamp,
  useSidebarInboxStore,
} from "@/stores/sidebar-inbox-store";
import { formatElapsed } from "@/stores/sidebar-density-store";
import { activateWorkspaceInteraction } from "@/lib/perf/instrumented-activate";
import { launchAgentChatPane } from "@/lib/agent-chat/launch-pane";
import { createEmptyWorkspace } from "@/tauri/commands";
import {
  getWorkspaceStatus,
  STATUS_LABEL,
  STATUS_TEXT_CLASS,
} from "@/lib/pane-status";
import { useCoarseClock } from "@/lib/use-coarse-clock";
import { useResolvedKeybinds } from "@/hooks/use-resolved-keybinds";
import type { ActivePaneStatus, WorkspaceSnapshot } from "@/tauri/types";
import { useProjectAppearance } from "./use-project-appearance";
import { WorkspaceInboxMenu } from "./workspace-inbox-menu";
import { WorkspaceHoverCard } from "./workspace-hover-card";
import { isRowActivationKey } from "./sidebar-row-activation";
import { SidebarNeedsYouStrip } from "./sidebar-needs-you-strip";
import { SidebarHeaderActions, useNewAgentAction } from "./sidebar-action-row";
import {
  SidebarProjectFilter,
  computeProjectCounts,
} from "./sidebar-project-filter";
import { PendingWorkspaceRow } from "./pending-workspace-row";
import {
  buildSidebarDraftCatalog,
  SidebarDraftBlock,
  useFrozenActiveDraftRow,
} from "./sidebar-draft-block";
import { formatTimeUntil } from "./sidebar-snooze";
import { setJumpTargets } from "./sidebar-inbox-jump";
import { isSnoozeable, isWorkspaceUnread } from "./sidebar-inbox";
import type { InboxRepo } from "./sidebar-inbox-card";
import { useSidebarViewStore } from "./sidebar-view-store";
import {
  buildProjectsModel,
  visibleLiveIds,
  type ProjectsRow,
  type ProjectsSection,
} from "./sidebar-projects-model";

/** Start a chat in a project — the same path the old project group's `+`
 *  took (`SidebarProjectGroup.handlePlusClick`) and Ctrl+Shift+N takes: a
 *  lazy project draft when agent chat is on, otherwise (or with Shift) the
 *  New Workspace dialog. */
async function newChatInProject(projectPath: string, shiftKey: boolean) {
  const ui = useUIStore.getState();
  const flags = useFeatureFlags.getState();
  if (shiftKey || !flags.enableAgentChat) {
    ui.setShowNewWorkspaceDialog(true, projectPath);
    return;
  }
  if (flags.enableLazyWorkspaceCreation) {
    const store = useChatDraftStore.getState();
    const draft = store.getOrCreateProjectDraft(projectPath);
    store.setActiveDraft(draft.draftId);
    return;
  }
  try {
    const wsId = await createEmptyWorkspace(projectPath);
    await activateWorkspaceInteraction(wsId);
    await launchAgentChatPane(wsId, null, projectPath);
  } catch (err) {
    console.error("[sidebar] failed to open chat pane:", err);
    ui.setShowNewWorkspaceDialog(true, projectPath);
  }
}

/** The hover `+` on the Chats and project headers, laid over the shared
 *  sidebar action primitives: 24px with a 14px glyph, hidden until its
 *  header is hovered or contains keyboard focus, and visible on touch. The
 *  primitive's enlarged touch target (`after:`) is dropped so it never
 *  overlaps the header's own toggle. */
const HEADER_PLUS_CLASS =
  "top-1 right-1 size-6 text-muted-foreground pointer-events-none opacity-0 transition-opacity duration-150 motion-reduce:transition-none after:hidden hover:bg-surface-3 hover:text-foreground focus-visible:pointer-events-auto focus-visible:opacity-100 pointer-coarse:pointer-events-auto pointer-coarse:opacity-100 [&>svg]:size-3.5";

/** Status glyph at the start of a row. Each state has its own shape, so the
 *  row reads correctly in any palette; colour only reinforces it. */
function StatusGlyph({
  status,
  snoozed,
}: {
  status: ActivePaneStatus | null;
  snoozed: boolean;
}) {
  if (status === "permission") {
    return (
      <span className="flex size-3.5 shrink-0 items-center justify-center">
        <span className="size-2 rounded-full bg-status-attention motion-safe:animate-pulse" />
      </span>
    );
  }
  if (status === "working") {
    // Static, like the inbox card's working mark: navigation reports
    // lifecycle, and the animated activity belongs inside the thread.
    return (
      <CircleDotDashed
        aria-hidden
        className="size-3.5 shrink-0 text-status-working"
      />
    );
  }
  if (status === "review") {
    return <Check aria-hidden className="size-3.5 shrink-0 text-status-open" />;
  }
  if (status === "monitoring") {
    return (
      <span className="flex size-3.5 shrink-0 items-center justify-center">
        <span className="size-2 rounded-full bg-status-monitoring" />
      </span>
    );
  }
  if (snoozed) {
    return (
      <AlarmClock
        aria-hidden
        className="size-3.5 shrink-0 text-muted-foreground/60"
      />
    );
  }
  return (
    <span className="flex size-3.5 shrink-0 items-center justify-center">
      <Circle aria-hidden className="size-2 text-muted-foreground/45" />
    </span>
  );
}

interface ProjectRowProps {
  workspace: WorkspaceSnapshot;
  repo: InboxRepo;
  status: ActivePaneStatus | null;
  isActive: boolean;
  unread: boolean;
  /** Right-side text when no live status speaks: relative time since work,
   *  or time until a snooze wakes. */
  meta: string | null;
  /** Parked rows (settled / snoozed) under "Show N settled". */
  parked: "settled" | "snoozed" | null;
}

/** One workspace row: status glyph, title, short status or time. Memoized
 *  with primitive props, so a status flip on one workspace re-renders only
 *  that row. */
const ProjectRow = memo(function ProjectRow({
  workspace,
  repo,
  status,
  isActive,
  unread,
  meta,
  parked,
}: ProjectRowProps) {
  const id = workspace.workspace_id;
  const activate = () => {
    activateWorkspaceInteraction(id).catch(console.error);
  };
  // Settle / snooze guardrail shared with the inbox: live or blocked work can
  // never be parked, and a pin overrides parking.
  const canPark = workspace.pinned_at == null && isSnoozeable(status);
  // Quiet when nothing here wants a human: an idle workspace you are not in
  // and have read. Hover restores it.
  const receded = !isActive && !unread && status === null;

  return (
    <WorkspaceInboxMenu
      workspace={workspace}
      repo={repo}
      settleAction={
        parked === "settled"
          ? {
              kind: "unsettle",
              onAction: () =>
                useSidebarInboxStore.getState().unsettle(id, "user"),
            }
          : parked === null && canPark
            ? {
                kind: "settle",
                onAction: () =>
                  useSidebarInboxStore
                    .getState()
                    .settle(id, workspace.last_active_at ?? undefined),
              }
            : undefined
      }
      snoozeAction={
        parked === "snoozed"
          ? {
              kind: "wake",
              offered: false,
              onSnooze: () => {},
              onWake: () =>
                useSidebarInboxStore.getState().unsnooze(id, "user"),
            }
          : parked === null && canPark
            ? {
                kind: "snooze",
                offered: true,
                onSnooze: (until) =>
                  useSidebarInboxStore.getState().snooze(id, until),
                onWake: () => {},
              }
            : undefined
      }
    >
      <div>
        <WorkspaceHoverCard workspace={workspace} repo={repo} status={status}>
          <div
            role="button"
            tabIndex={0}
            data-ws-id={id}
            data-projects-row={id}
            data-parked={parked ?? undefined}
            aria-current={isActive ? "page" : undefined}
            onClick={activate}
            onKeyDown={(e) => {
              if (!isRowActivationKey(e)) return;
              if (e.key === " ") e.preventDefault();
              activate();
            }}
            className={cn(
              "group/row flex h-8 min-w-0 cursor-pointer items-center gap-2 rounded-lg px-2",
              "[content-visibility:auto] [contain-intrinsic-size:auto_32px]",
              "outline-none transition-colors duration-150 hover:bg-surface-2 focus-visible:bg-surface-2",
              isActive && "bg-surface-3",
            )}
          >
            <StatusGlyph status={status} snoozed={parked === "snoozed"} />
            {unread && <span className="sr-only">Unread: </span>}
            <span
              className={cn(
                "min-w-0 flex-1 truncate text-body transition-colors duration-150",
                unread ? "font-semibold" : "font-medium",
                receded || parked
                  ? "text-muted-foreground group-hover/row:text-foreground group-focus-visible/row:text-foreground"
                  : "text-foreground",
              )}
            >
              {workspace.title}
            </span>
            {status !== null && status !== "working" ? (
              <span
                className={cn(
                  "shrink-0 text-label font-semibold",
                  STATUS_TEXT_CLASS[status],
                )}
              >
                {STATUS_LABEL[status]}
              </span>
            ) : status === "working" ? (
              <span className="shrink-0 text-label font-semibold text-status-working">
                <span role="status">{STATUS_LABEL.working}</span>
              </span>
            ) : meta ? (
              <span
                aria-label={parked === "snoozed" ? `Wakes in ${meta}` : undefined}
                className="shrink-0 font-mono text-label tabular-nums text-muted-foreground/70"
              >
                {meta}
              </span>
            ) : null}
          </div>
        </WorkspaceHoverCard>
      </div>
    </WorkspaceInboxMenu>
  );
});

/** Project avatar for a group header, resolving the project's appearance. */
function GroupAvatar({ name, path }: { name: string; path: string }) {
  const appearance = useProjectAppearance(path);
  return (
    <ProjectAvatar
      name={name}
      color={appearance.customColor}
      imageUrl={appearance.imageUrl}
      cacheBust={appearance.imageVersion}
      size="md"
      shape="square"
      className="font-bold"
    />
  );
}

function parkedLabel(settled: number, snoozed: number): string {
  if (snoozed === 0) return `${settled} settled`;
  if (settled === 0) return `${snoozed} snoozed`;
  return `${settled + snoozed} settled or snoozed`;
}

export function SidebarProjectsView() {
  const workspaces = useAppStore((s) => s.appState?.workspaces);
  const paneStatuses = useAppStore((s) => s.appState?.pane_statuses);
  const allWorkspaces = useMemo(() => workspaces ?? [], [workspaces]);
  const activeDraftId = useChatDraftStore((s) => s.activeDraftId);
  const selectedWorkspaceId = useAppStore(selectActiveWorkspaceId) ?? "";
  const activeWorkspaceId = activeDraftId ? "" : selectedWorkspaceId;
  const homeDir = useHomeDir();
  const hosts = useHosts();
  const projectGroups = useProjectGroupedWorkspaces(allWorkspaces, homeDir, hosts);
  const sidebarDraftCatalog = useMemo(
    () => buildSidebarDraftCatalog(projectGroups, homeDir),
    [projectGroups, homeDir],
  );
  const pendingWorkspaces = useUIStore((s) => s.pendingWorkspaces);
  const expandProjectRequest = useUIStore((s) => s.expandProjectRequest);

  const loadInbox = useSidebarInboxStore((s) => s.load);
  const settled = useSidebarInboxStore((s) => s.settled);
  const snoozed = useSidebarInboxStore((s) => s.snoozed);
  const filter = useSidebarInboxStore((s) => s.filter);
  const setFilter = useSidebarInboxStore((s) => s.setFilter);
  const frozenActiveDraftRow = useFrozenActiveDraftRow(sidebarDraftCatalog, filter);

  const folded = useSidebarViewStore((s) => s.folded);
  const setFolded = useSidebarViewStore((s) => s.setFolded);

  const now = useCoarseClock(true);

  useEffect(() => {
    void loadInbox();
  }, [loadInbox]);

  // ── Derived data (memoized: the grouping only moves when its inputs do) ──

  const settledIds = useMemo(() => new Set(settled.map((e) => e.id)), [settled]);
  const snoozedIds = useMemo(() => new Set(snoozed.map((e) => e.id)), [snoozed]);
  const settledById = useMemo(
    () => new Map(settled.map((e) => [e.id, e] as const)),
    [settled],
  );
  const snoozedById = useMemo(
    () => new Map(snoozed.map((e) => [e.id, e] as const)),
    [snoozed],
  );
  const parkedIds = useMemo(
    () => new Set([...settledIds, ...snoozedIds]),
    [settledIds, snoozedIds],
  );

  const statusById = useMemo(() => {
    const map = new Map<string, ActivePaneStatus | null>();
    for (const ws of allWorkspaces) {
      map.set(
        ws.workspace_id,
        paneStatuses ? getWorkspaceStatus(ws.surfaces, paneStatuses) : null,
      );
    }
    return map;
  }, [allWorkspaces, paneStatuses]);

  // The lifecycle writes (resurface, wake, the wake timer) are made by
  // `useInboxLifecycle`, which runs whichever view is mounted. This override
  // only covers the frame between a status landing and that effect's store
  // write: a parked workspace whose agent is working or blocked shows as live
  // at once instead of flashing hidden first.
  const liveOverride = useMemo(() => {
    const out = new Set<string>();
    for (const id of parkedIds) {
      const status = statusById.get(id) ?? null;
      if (status === "working" || status === "permission") out.add(id);
    }
    return out;
  }, [parkedIds, statusById]);

  const storedIndex = useMemo(() => {
    const map = new Map<string, number>();
    allWorkspaces.forEach((ws, i) => map.set(ws.workspace_id, i));
    return map;
  }, [allWorkspaces]);

  const model = useMemo(
    () =>
      buildProjectsModel({
        projectGroups,
        homeDir,
        storedIndex,
        parkedIds,
        liveOverride,
        filter,
      }),
    [projectGroups, homeDir, storedIndex, parkedIds, liveOverride, filter],
  );

  // Interned `{name, path}` per project so memoized rows keep a stable `repo`.
  const repoIntern = useRef(new Map<string, InboxRepo>());
  const repoByWorkspace = useMemo(() => {
    const intern = repoIntern.current;
    const map = new Map<string, InboxRepo>();
    for (const group of projectGroups) {
      const key = `${group.projectName}\n${group.projectPath}`;
      let repo = intern.get(key);
      if (!repo) {
        repo = { name: group.projectName, path: group.projectPath };
        intern.set(key, repo);
      }
      for (const ws of group.workspaces) map.set(ws.workspace_id, repo);
    }
    return map;
  }, [projectGroups]);

  const projectCounts = useMemo(
    () => computeProjectCounts(projectGroups, settledIds, snoozedIds),
    [projectGroups, settledIds, snoozedIds],
  );
  const filterName = filter
    ? projectGroups.find((g) => g.projectPath === filter)?.projectName ?? null
    : null;

  // Same fallback as the inbox: a filtered project that vanished resets the
  // filter instead of leaving an unexplained empty list.
  const appStateLoaded = useAppStore((s) => s.appState !== null);
  useEffect(() => {
    if (!appStateLoaded || filter === null) return;
    if (projectGroups.some((g) => g.projectPath === filter)) return;
    setFilter(null);
  }, [appStateLoaded, filter, projectGroups, setFilter]);

  // The Needs-you strip asks a folded group to open before it scrolls to the
  // blocked row. Consume the one-shot request.
  useEffect(() => {
    if (!expandProjectRequest) return;
    if (folded[expandProjectRequest]) setFolded(expandProjectRequest, false);
    useUIStore.getState().clearExpandProjectRequest(expandProjectRequest);
  }, [expandProjectRequest, folded, setFolded]);

  // Alt+1..9 targets the visible live rows in this view's order.
  const jumpKey = visibleLiveIds(model, folded).join(" ");
  useEffect(() => {
    setJumpTargets(jumpKey ? jumpKey.split(" ") : []);
  }, [jumpKey]);
  useEffect(() => () => setJumpTargets([]), []);

  // "Show N settled" disclosure, per project. Session-only, like the inbox's
  // Settled shelf, which also starts closed.
  const [openParked, setOpenParked] = useState<ReadonlySet<string>>(new Set());
  const toggleParked = useCallback((path: string) => {
    setOpenParked((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  }, []);

  const statusOf = (id: string) => statusById.get(id) ?? null;
  const unreadOf = (ws: WorkspaceSnapshot) =>
    ws.workspace_id !== activeWorkspaceId &&
    isWorkspaceUnread(ws.last_active_at, ws.last_visited_at);
  const idleMeta = (ws: WorkspaceSnapshot) =>
    ws.last_active_at != null ? formatElapsed(now - ws.last_active_at) : null;

  // Rows are sidebar menu items; follow-up children sit in a
  // `SidebarMenuSub`, which draws the guide line. The sub list is pulled
  // back to the old geometry (15px indent, 6px gutter, no gap) so a child
  // row keeps its full 32px height and width.
  const renderRows = (
    rows: readonly ProjectsRow[],
    nested = false,
  ): React.ReactNode =>
    rows.map((row) => {
      const ws = row.workspace;
      const repo = repoByWorkspace.get(ws.workspace_id);
      if (!repo) return null;
      const Item = nested ? SidebarMenuSubItem : SidebarMenuItem;
      return (
        <Item key={ws.workspace_id}>
          <ProjectRow
            workspace={ws}
            repo={repo}
            status={statusOf(ws.workspace_id)}
            isActive={ws.workspace_id === activeWorkspaceId}
            unread={unreadOf(ws)}
            meta={idleMeta(ws)}
            parked={null}
          />
          {row.children.length > 0 && (
            <SidebarMenuSub
              data-ws-children={ws.workspace_id}
              className="mx-0 ml-[15px] translate-x-0 gap-0 border-border/70 px-0 py-0 pl-1.5"
            >
              {renderRows(row.children, true)}
            </SidebarMenuSub>
          )}
        </Item>
      );
    });

  const renderParked = (section: ProjectsSection) => {
    if (section.parked.length === 0) return null;
    const open = openParked.has(section.path);
    // The open workspace never hides behind a closed toggle.
    const visible = open
      ? section.parked
      : section.parked.filter((ws) => ws.workspace_id === activeWorkspaceId);
    const settledCount = section.parked.filter((ws) =>
      settledIds.has(ws.workspace_id),
    ).length;
    const snoozedCount = section.parked.length - settledCount;
    return (
      <>
        {visible.map((ws) => {
          const repo = repoByWorkspace.get(ws.workspace_id);
          if (!repo) return null;
          const snoozeEntry = snoozedById.get(ws.workspace_id);
          const settledEntry = settledById.get(ws.workspace_id);
          const meta = snoozeEntry
            ? formatTimeUntil(snoozeEntry.until - now)
            : settledEntry
              ? formatElapsed(now - resolveSettledTimestamp(settledEntry))
              : null;
          return (
            <SidebarMenuItem key={ws.workspace_id}>
              <ProjectRow
                workspace={ws}
                repo={repo}
                status={statusOf(ws.workspace_id)}
                isActive={ws.workspace_id === activeWorkspaceId}
                unread={unreadOf(ws)}
                meta={meta}
                parked={snoozeEntry ? "snoozed" : "settled"}
              />
            </SidebarMenuItem>
          );
        })}
        <SidebarMenuItem>
          <button
            type="button"
            data-parked-toggle={section.path}
            aria-expanded={open}
            onClick={() => toggleParked(section.path)}
            className="flex h-7 w-full items-center rounded-lg pl-[30px] text-left text-label text-muted-foreground/80 transition-colors duration-150 hover:text-foreground"
          >
            {open ? "Hide" : "Show"} {parkedLabel(settledCount, snoozedCount)}
          </button>
        </SidebarMenuItem>
      </>
    );
  };

  const pendingByProject = useMemo(() => {
    const map = new Map<string, typeof pendingWorkspaces>();
    for (const pw of pendingWorkspaces) {
      map.set(pw.projectPath, [...(map.get(pw.projectPath) ?? []), pw]);
    }
    return map;
  }, [pendingWorkspaces]);

  const hasNeedsYou = (rows: readonly ProjectsRow[]): boolean =>
    rows.some(
      (r) => statusOf(r.workspace.workspace_id) === "permission" || hasNeedsYou(r.children),
    );

  const isEmpty =
    model.chats === null &&
    model.projects.length === 0 &&
    pendingWorkspaces.length === 0;

  return (
    <div className="flex flex-1 flex-col" data-sidebar-projects-view>
      <div className="sticky top-0 z-10 bg-sidebar">
        <div className="flex min-w-0 items-center gap-1 px-2.5 pb-2.5">
          <SidebarProjectFilter
            projectGroups={projectGroups}
            counts={projectCounts}
            filter={filter}
            filterName={filterName}
            setFilter={setFilter}
          />
          <SidebarHeaderActions />
        </div>
        <SidebarNeedsYouStrip projectGroups={projectGroups} filterPath={filter} />
      </div>

      <div className="flex flex-1 flex-col px-2.5 pb-2.5">
        <SidebarDraftBlock
          catalog={sidebarDraftCatalog}
          filterPath={filter}
          frozenActive={frozenActiveDraftRow}
        />

        {isEmpty && (
          <p className="px-2 py-6 text-center text-label text-muted-foreground">
            {filterName ? `No workspaces in ${filterName}` : "No workspaces yet"}
          </p>
        )}

        {model.chats && (
          <SidebarGroup
            role="region"
            aria-label="Chats"
            data-projects-chats
            className="p-0 pb-2"
          >
            <ChatsHeader />
            <SidebarMenu>
              {renderRows(model.chats.rows)}
              {renderParked(model.chats)}
            </SidebarMenu>
          </SidebarGroup>
        )}

        {model.projects.length > 0 && (
          <SidebarGroup role="region" aria-label="Projects" className="p-0">
            <SidebarGroupLabel className="h-7 pt-1">
              <Eyebrow>Projects</Eyebrow>
            </SidebarGroupLabel>
            <SidebarMenu>
              {model.projects.map((project) => {
                const isFolded = folded[project.path] === true;
                const pending = pendingByProject.get(project.path) ?? [];
                return (
                  <SidebarMenuItem
                    key={project.path}
                    data-project-group={project.path}
                  >
                    <ProjectGroupHeader
                      name={project.name}
                      path={project.path}
                      count={project.liveCount}
                      folded={isFolded}
                      needsYou={isFolded && hasNeedsYou(project.rows)}
                      onToggle={setFolded}
                    />
                    {!isFolded && (
                      <SidebarMenu className="pb-1">
                        {renderRows(project.rows)}
                        {pending.map((pw) => (
                          <SidebarMenuItem key={pw.id}>
                            <PendingWorkspaceRow
                              pending={pw}
                              className="gap-2 px-2 py-2"
                            />
                          </SidebarMenuItem>
                        ))}
                        {renderParked(project)}
                      </SidebarMenu>
                    )}
                  </SidebarMenuItem>
                );
              })}
            </SidebarMenu>
          </SidebarGroup>
        )}
      </div>
    </div>
  );
}

/** "Chats" section header with a hover `+` for a new home-directory chat —
 *  the same gesture as the header's New agent button. */
function ChatsHeader() {
  const handleNewAgent = useNewAgentAction();
  const { getKeysForAction } = useResolvedKeybinds();
  const keys = getKeysForAction("newAgent");
  return (
    <SidebarGroupLabel className="group/chats relative h-7 pt-1">
      <Eyebrow className="flex-1">Chats</Eyebrow>
      <Tooltip>
        <TooltipTrigger asChild>
          <SidebarGroupAction
            aria-label="New chat"
            onClick={handleNewAgent}
            className={cn(
              HEADER_PLUS_CLASS,
              "right-2 hover:bg-surface-2 group-hover/chats:pointer-events-auto group-hover/chats:opacity-100 group-focus-within/chats:pointer-events-auto group-focus-within/chats:opacity-100",
            )}
          >
            <Plus />
          </SidebarGroupAction>
        </TooltipTrigger>
        <TooltipContent side="bottom" sideOffset={4} className="text-label">
          New chat in home directory{keys ? ` · ${keys}` : ""}
        </TooltipContent>
      </Tooltip>
    </SidebarGroupLabel>
  );
}

const ProjectGroupHeader = memo(function ProjectGroupHeader({
  name,
  path,
  count,
  folded,
  needsYou,
  onToggle,
}: {
  name: string;
  path: string;
  count: number;
  folded: boolean;
  /** Folded and hiding a workspace that is waiting on the user. */
  needsYou: boolean;
  onToggle: (path: string, folded: boolean) => void;
}) {
  const { getKeysForAction } = useResolvedKeybinds();
  const newKeys = getKeysForAction("newWorkspaceInProject");
  return (
    <div className="group/proj relative grid h-8 grid-cols-[minmax(0,1fr)_auto] items-center rounded-lg pr-1 transition-colors duration-150 hover:bg-surface-2">
      <button
        type="button"
        aria-expanded={!folded}
        aria-label={`${name} (${count})`}
        data-project-header={path}
        onClick={() => onToggle(path, !folded)}
        className="col-start-1 row-start-1 flex h-full min-w-0 items-center gap-2 rounded-lg pl-1.5 text-left outline-none focus-visible:bg-surface-2"
      >
        <ChevronRight
          aria-hidden
          className={cn(
            "size-3.5 shrink-0 text-muted-foreground/70 transition-transform duration-150",
            !folded && "rotate-90",
          )}
        />
        <GroupAvatar name={name} path={path} />
        <span className="min-w-0 truncate text-body font-semibold text-foreground">
          {name}
        </span>
        {needsYou && (
          <span
            data-folded-needs-you
            title="A workspace in this project needs you"
            className="flex shrink-0 items-center"
          >
            <span className="size-1.5 rounded-full bg-status-attention motion-safe:animate-pulse" />
            <span className="sr-only">A workspace in this project needs you</span>
          </span>
        )}
      </button>
      {/* The count and create action share a cell, outside the fold toggle. */}
      <span
        aria-hidden
        className="pointer-events-none col-start-2 row-start-1 min-w-6 px-1 text-center text-label tabular-nums text-muted-foreground/70 transition-opacity duration-150 motion-reduce:transition-none group-hover/proj:opacity-0 group-focus-within/proj:opacity-0 pointer-coarse:opacity-0"
      >
        {count}
      </span>
      <Tooltip>
        <TooltipTrigger asChild>
          <SidebarMenuAction
            aria-label={`New chat in ${name}`}
            onClick={(e) => void newChatInProject(path, e.shiftKey)}
            className={cn(
              HEADER_PLUS_CLASS,
              "static col-start-2 row-start-1 justify-self-center group-hover/proj:pointer-events-auto group-hover/proj:opacity-100 group-focus-within/proj:pointer-events-auto group-focus-within/proj:opacity-100",
            )}
          >
            <Plus />
          </SidebarMenuAction>
        </TooltipTrigger>
        <TooltipContent side="bottom" sideOffset={4} className="text-label">
          New chat in {name} · Shift+click for workspace dialog
          {newKeys ? ` · ${newKeys}` : ""}
        </TooltipContent>
      </Tooltip>
    </div>
  );
});
