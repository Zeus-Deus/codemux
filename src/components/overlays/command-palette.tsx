import { useEffect, useMemo, useRef, useState } from "react";
import { Command as CommandPrimitive } from "cmdk";
import {
  ArrowLeft,
  ArrowRight,
  Columns2,
  FolderOpen,
  Globe,
  Keyboard,
  LayoutGrid,
  PanelLeft,
  Play,
  Plus,
  RefreshCw,
  Search,
  Settings,
  SplitSquareHorizontal,
  SplitSquareVertical,
  Terminal,
  X,
} from "lucide-react";
import { CommandDialog } from "@/components/ui/command";
import { ProjectAvatar } from "@/components/ui/project-avatar";
import {
  useAppStore,
  useHomeDir,
  useProjectGroupedWorkspaces,
  type ProjectGroup,
} from "@/stores/app-store";
import { useHosts } from "@/stores/hosts-store";
import { useUIStore } from "@/stores/ui-store";
import { useSidebarInboxStore } from "@/stores/sidebar-inbox-store";
import {
  formatElapsed,
  useSidebarDensityStore,
} from "@/stores/sidebar-density-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useProjectAppearance } from "@/components/layout/use-project-appearance";
import { useCoarseClock } from "@/lib/use-coarse-clock";
import {
  getWorkspaceStatus,
  STATUS_DOT_CLASS,
  STATUS_LABEL,
  STATUS_TEXT_CLASS,
} from "@/lib/pane-status";
import { shortenPath } from "@/lib/shorten-path";
import { cn } from "@/lib/utils";
import {
  activateWorkspace,
  createBrowserPane,
  cyclePane,
  getPresets,
  regenerateMcpConfig,
  setPresetBarVisible,
} from "@/tauri/commands";
import { dispatch } from "@/hooks/use-keyboard-shortcuts";
import { useResolvedKeybinds } from "@/hooks/use-resolved-keybinds";
import type { ActivePaneStatus, WorkspaceSnapshot } from "@/tauri/types";
import {
  COMMAND_MODE_PREFIX,
  commandSearchText,
  compareWorkspaceOrder,
  groupCountLabel,
  parsePaletteQuery,
  rankByQuery,
  resultCountLabel,
  workspacePathText,
  workspaceRowSubtitle,
  workspaceSearchText,
} from "./command-palette-model";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/** Resting caps. The palette is a jump list, not a browser: without a query it
 *  shows the handful of things you're most likely to want, and searching opens
 *  it up. Whatever is hidden is reported in the group header count, never
 *  silently dropped. */
const WORKSPACE_CAP_RESTING = 8;
const WORKSPACE_CAP_SEARCH = 24;
const PROJECT_CAP_RESTING = 4;
const PROJECT_CAP_SEARCH = 8;

// ── Command catalogue ────────────────────────────────────────────────────

/**
 * A palette command. `actionId` does double duty: it resolves the keybind hint
 * AND, when no explicit `run` is given, routes the command through the same
 * `dispatch()` the global keyboard handler uses — so a shortcut and its
 * palette row can never drift apart.
 */
interface PaletteCommand {
  id: string;
  label: string;
  icon: React.ComponentType<{ className?: string }>;
  /** Hidden search synonyms. */
  keywords?: string;
  /** Keybind-registry action id, for the hint and the default runner. */
  actionId?: string;
  /** Overrides `actionId` dispatch when the action has no keybind entry. */
  run?: (ctx: CommandContext) => void;
  /** Hidden when there is no active workspace to act on. */
  requiresWorkspace?: boolean;
}

interface CommandContext {
  workspace: WorkspaceSnapshot | null;
  activePaneId: string | undefined;
}

const COMMANDS: PaletteCommand[] = [
  // Workspaces & projects
  { id: "new-agent", label: "New agent", icon: Plus, actionId: "newAgent", keywords: "chat thread start" },
  {
    id: "new-workspace-project",
    label: "New workspace in this project",
    icon: Plus,
    actionId: "newWorkspaceInProject",
    requiresWorkspace: true,
  },
  {
    id: "new-workspace",
    label: "Create new workspace",
    icon: Plus,
    keywords: "add",
    run: () => useUIStore.getState().setShowNewWorkspaceDialog(true),
  },
  { id: "open-project", label: "Open project…", icon: FolderOpen, actionId: "openProject", keywords: "folder directory" },
  {
    id: "run-dev",
    label: "Run dev command",
    icon: Play,
    actionId: "runDevCommand",
    keywords: "server start npm",
    requiresWorkspace: true,
  },
  { id: "next-workspace", label: "Next workspace", icon: ArrowRight, actionId: "nextWorkspace" },
  { id: "prev-workspace", label: "Previous workspace", icon: ArrowLeft, actionId: "prevWorkspace" },

  // Search
  { id: "file-search", label: "Find file by name", icon: Search, actionId: "fileSearch", keywords: "goto open" },
  { id: "content-search", label: "Search in files", icon: Search, actionId: "contentSearch", keywords: "grep ripgrep find text" },

  // Panes & tabs
  { id: "split-right", label: "Split pane right", icon: SplitSquareHorizontal, actionId: "splitPaneRight", requiresWorkspace: true },
  { id: "split-down", label: "Split pane down", icon: SplitSquareVertical, actionId: "splitPaneDown", requiresWorkspace: true },
  { id: "close-pane", label: "Close pane", icon: X, actionId: "closePane", requiresWorkspace: true },
  { id: "new-tab", label: "New terminal tab", icon: Terminal, actionId: "newTab", keywords: "shell console", requiresWorkspace: true },
  { id: "close-tab", label: "Close tab", icon: X, actionId: "closeTab", requiresWorkspace: true },
  {
    id: "focus-next-pane",
    label: "Focus next pane",
    icon: LayoutGrid,
    requiresWorkspace: true,
    run: () => void cyclePane(1).catch(console.error),
  },
  {
    id: "focus-prev-pane",
    label: "Focus previous pane",
    icon: LayoutGrid,
    requiresWorkspace: true,
    run: () => void cyclePane(-1).catch(console.error),
  },
  {
    id: "open-browser",
    label: "Open browser",
    icon: Globe,
    keywords: "web preview localhost",
    requiresWorkspace: true,
    run: ({ activePaneId }) => {
      if (activePaneId) void createBrowserPane(activePaneId).catch(console.error);
    },
  },

  // View
  { id: "toggle-sidebar", label: "Toggle sidebar", icon: PanelLeft, actionId: "toggleSidebar" },
  { id: "toggle-right-panel", label: "Toggle right panel", icon: Columns2, actionId: "toggleRightPanel", requiresWorkspace: true },
  {
    id: "toggle-preset-bar",
    label: "Toggle preset bar",
    icon: Columns2,
    run: () =>
      void getPresets()
        .then((s) => setPresetBarVisible(!s.bar_visible))
        .catch(console.error),
  },
  { id: "shortcuts", label: "Keyboard shortcuts", icon: Keyboard, actionId: "showShortcuts", keywords: "keybinds bindings" },
  { id: "settings", label: "Settings", icon: Settings, actionId: "openSettings", keywords: "preferences config" },
  {
    id: "regen-mcp",
    label: "Regenerate MCP config",
    icon: RefreshCw,
    requiresWorkspace: true,
    run: ({ workspace }) => {
      if (workspace) void regenerateMcpConfig(workspace.workspace_id).catch(console.error);
    },
  },
];

// ── Rows ─────────────────────────────────────────────────────────────────

interface WorkspaceRow {
  kind: "workspace";
  key: string;
  workspace: WorkspaceSnapshot;
  projectName: string;
  projectPath: string;
  status: ActivePaneStatus | null;
  parked: boolean;
  activityAt: number | undefined;
}

interface ProjectRow {
  kind: "project";
  key: string;
  group: ProjectGroup;
  activeCount: number;
}

interface CommandRow {
  kind: "command";
  key: string;
  command: PaletteCommand;
  keys: string;
}

export function CommandPalette({ open, onOpenChange }: Props) {
  return (
    <CommandDialog
      open={open}
      onOpenChange={onOpenChange}
      title="Command palette"
      description="Search workspaces, projects, and commands."
      className="top-24 w-full gap-0 border border-border p-0 sm:max-w-[640px]"
    >
      {/* Radix unmounts dialog content while closed, so the body's stores,
          clock, and avatar loads cost nothing until the palette opens. */}
      <PaletteBody onOpenChange={onOpenChange} />
    </CommandDialog>
  );
}

function PaletteBody({ onOpenChange }: { onOpenChange: (open: boolean) => void }) {
  const [rawQuery, setRawQuery] = useState("");
  const query = useMemo(() => parsePaletteQuery(rawQuery), [rawQuery]);
  const listRef = useRef<HTMLDivElement>(null);

  // Every keystroke re-ranks the list and cmdk re-selects the top row, but it
  // only scrolls on arrow navigation — so without this the user can be left
  // staring at the middle of a list whose selection is far above.
  useEffect(() => {
    listRef.current?.scrollTo({ top: 0 });
  }, [rawQuery]);

  const appState = useAppStore((s) => s.appState);
  const homeDir = useHomeDir();
  const hosts = useHosts();
  const { getKeysForAction } = useResolvedKeybinds();
  const now = useCoarseClock(true);

  const settled = useSidebarInboxStore((s) => s.settled);
  const snoozed = useSidebarInboxStore((s) => s.snoozed);
  const loadInbox = useSidebarInboxStore((s) => s.load);
  const setProjectFilter = useSidebarInboxStore((s) => s.setFilter);
  const statusSince = useSidebarDensityStore((s) => s.statusSince);
  const settledAt = useSidebarDensityStore((s) => s.settledAt);

  // The shelves decide ordering and the project counts, so the palette can't
  // wait for the sidebar to be the thing that hydrates them. `load` is
  // memoized store-side, so this is a no-op once the inbox has read its blob.
  useEffect(() => {
    void loadInbox();
  }, [loadInbox]);

  const workspaces = useMemo(
    () => appState?.workspaces ?? [],
    [appState?.workspaces],
  );
  const projectGroups = useProjectGroupedWorkspaces(workspaces, homeDir, hosts);

  const parkedIds = useMemo(() => {
    const ids = new Set<string>();
    for (const entry of settled) ids.add(entry.id);
    for (const entry of snoozed) ids.add(entry.id);
    return ids;
  }, [settled, snoozed]);

  const activeWorkspace = useMemo(
    () =>
      appState?.workspaces.find(
        (w) => w.workspace_id === appState.active_workspace_id,
      ) ?? null,
    [appState],
  );
  const activePaneId = activeWorkspace?.surfaces.find(
    (s) => s.surface_id === activeWorkspace.active_surface_id,
  )?.active_pane_id;

  // ── Workspace rows ─────────────────────────────────────────────────────
  const workspaceRows = useMemo<WorkspaceRow[]>(() => {
    const paneStatuses = appState?.pane_statuses;
    const repoOf = new Map<string, { name: string; path: string }>();
    for (const group of projectGroups) {
      for (const ws of group.workspaces) {
        repoOf.set(ws.workspace_id, { name: group.projectName, path: group.projectPath });
      }
    }
    const rows = workspaces.map((ws) => {
      const id = ws.workspace_id;
      const status = paneStatuses ? getWorkspaceStatus(ws.surfaces, paneStatuses) : null;
      const repo = repoOf.get(id);
      // Prefer the moment this workspace entered its current state, then the
      // sidebar's settle stamp, then the backend's activity stamp — the same
      // ladder the inbox card's age label walks.
      const activityAt =
        statusSince[id]?.at ?? settledAt[id] ?? ws.last_active_at ?? undefined;
      return {
        kind: "workspace" as const,
        key: `ws:${id}`,
        workspace: ws,
        projectName: repo?.name ?? ws.title,
        projectPath: repo?.path ?? ws.project_root ?? ws.cwd,
        status,
        parked: parkedIds.has(id),
        activityAt,
      };
    });
    return rows.sort(compareWorkspaceOrder);
    // `now` is deliberately absent: the coarse clock returns a fresh value on
    // every render, so depending on it here would re-map and re-sort every
    // workspace on every keystroke. Nothing above is time-derived — the idle
    // age is formatted at render time by the row itself.
  }, [workspaces, projectGroups, appState?.pane_statuses, statusSince, settledAt, parkedIds]);

  // ── Project rows ───────────────────────────────────────────────────────
  const projectRows = useMemo<ProjectRow[]>(
    () =>
      projectGroups.map((group) => ({
        kind: "project" as const,
        key: `proj:${group.projectPath}`,
        group,
        activeCount: group.workspaces.filter((w) => !parkedIds.has(w.workspace_id)).length,
      })),
    [projectGroups, parkedIds],
  );

  // ── Command rows ───────────────────────────────────────────────────────
  const commandRows = useMemo<CommandRow[]>(
    () =>
      COMMANDS.filter((c) => !c.requiresWorkspace || activeWorkspace !== null).map((command) => ({
        kind: "command" as const,
        key: `cmd:${command.id}`,
        command,
        keys: command.actionId ? getKeysForAction(command.actionId) : "",
      })),
    [activeWorkspace, getKeysForAction],
  );

  // ── Filter + rank ──────────────────────────────────────────────────────
  const commandsOnly = query.mode === "commands";

  const matchedWorkspaces = useMemo(
    () =>
      commandsOnly
        ? []
        : rankByQuery(
            workspaceRows,
            query,
            (r) => workspaceSearchText(r.workspace),
            (r) => workspacePathText(r.workspace),
          ),
    [workspaceRows, query, commandsOnly],
  );
  const matchedProjects = useMemo(
    () =>
      commandsOnly
        ? []
        : rankByQuery(
            projectRows,
            query,
            (r) => r.group.projectName,
            (r) => r.group.projectPath,
          ),
    [projectRows, query, commandsOnly],
  );
  const matchedCommands = useMemo(
    () =>
      // A path query is asking "where", and no command has a location — so
      // outside explicit command mode, path mode drops them rather than
      // letting a long `a/b/c` subsequence-match some unrelated label.
      query.pathMode && !commandsOnly
        ? []
        : rankByQuery(
            commandRows,
            query,
            (r) => commandSearchText(r.command),
            (r) => commandSearchText(r.command),
          ),
    [commandRows, query, commandsOnly],
  );

  const searching = query.needle !== "";
  const shownWorkspaces = matchedWorkspaces.slice(
    0,
    searching ? WORKSPACE_CAP_SEARCH : WORKSPACE_CAP_RESTING,
  );
  const shownProjects = matchedProjects.slice(
    0,
    searching ? PROJECT_CAP_SEARCH : PROJECT_CAP_RESTING,
  );
  const totalShown =
    shownWorkspaces.length + shownProjects.length + matchedCommands.length;

  // ── Actions ────────────────────────────────────────────────────────────
  const close = () => onOpenChange(false);

  const openWorkspace = (workspaceId: string) => {
    close();
    // Mirrors the inbox card: clear any active chat draft first so activating
    // a workspace isn't overridden by a draft surface still holding the view.
    useChatDraftStore.getState().setActiveDraft(null);
    activateWorkspace(workspaceId).catch(console.error);
  };

  const openProject = (row: ProjectRow) => {
    close();
    // "Go to this project": scope the sidebar inbox to it, then land on its
    // top-ranked workspace — whatever is asking for you there, falling back to
    // the most recently active. The palette's answer to "take me back to what
    // I was doing over there".
    setProjectFilter(row.group.projectPath);
    const target = workspaceRows.find((r) => r.projectPath === row.group.projectPath);
    if (target) openWorkspace(target.workspace.workspace_id);
  };

  const runCommand = (command: PaletteCommand) => {
    close();
    if (command.run) {
      command.run({ workspace: activeWorkspace, activePaneId });
      return;
    }
    if (command.actionId) dispatch(command.actionId);
  };

  return (
    <CommandPrimitive
      shouldFilter={false}
      loop
      className="flex w-full flex-col overflow-hidden bg-popover text-popover-foreground"
    >
      {/* Header */}
      <div className="flex h-[52px] flex-none items-center gap-2.5 border-b border-border/60 pr-3.5 pl-4">
        <Search className="size-[15px] shrink-0 text-muted-foreground/70" />
        {commandsOnly && (
          <span className="flex h-[22px] flex-none items-center rounded-md bg-accent-ember/15 px-2 font-mono text-[10px] tracking-wide text-accent-ember">
            Commands
          </span>
        )}
        <CommandPrimitive.Input
          autoFocus
          value={rawQuery}
          onValueChange={setRawQuery}
          placeholder={`Search workspaces, projects, commands…  (${COMMAND_MODE_PREFIX} for commands)`}
          className="h-8 min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground/70"
        />
        <kbd className="flex-none rounded-[5px] border border-border/60 px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground/70">
          esc
        </kbd>
      </div>

      {/* Results */}
      <div className="relative">
        <CommandPrimitive.List
          ref={listRef}
          className="thin-scrollbar max-h-[352px] scroll-py-8 overflow-x-hidden overflow-y-auto py-1.5 pr-0.5 pl-1.5"
        >
          {totalShown === 0 && (
            <div className="flex flex-col items-center gap-1.5 px-5 py-11 text-center">
              <span className="text-[13px] text-muted-foreground">
                {query.needle === "" ? (
                  "Nothing to show yet"
                ) : (
                  <>
                    No matches for{" "}
                    <span className="font-mono text-foreground">{query.needle}</span>
                  </>
                )}
              </span>
              <span className="text-[11px] text-muted-foreground/70">
                Try a branch name, a repo, or{" "}
                <span className="font-mono">{COMMAND_MODE_PREFIX}</span> for commands
              </span>
            </div>
          )}

          {shownWorkspaces.length > 0 && (
            <GroupHeader
              label="Workspaces"
              count={groupCountLabel(shownWorkspaces.length, matchedWorkspaces.length)}
              first
            />
          )}
          {shownWorkspaces.map((row) => (
            <WorkspaceItem
              key={row.key}
              row={row}
              now={now}
              onSelect={() => openWorkspace(row.workspace.workspace_id)}
            />
          ))}

          {shownProjects.length > 0 && (
            <GroupHeader
              label="Projects"
              count={groupCountLabel(shownProjects.length, matchedProjects.length)}
              first={shownWorkspaces.length === 0}
            />
          )}
          {shownProjects.map((row) => (
            <ProjectItem key={row.key} row={row} homeDir={homeDir} onSelect={() => openProject(row)} />
          ))}

          {matchedCommands.length > 0 && (
            <GroupHeader
              label="Commands"
              count={`${matchedCommands.length}`}
              first={shownWorkspaces.length === 0 && shownProjects.length === 0}
            />
          )}
          {matchedCommands.map((row) => (
            <CommandItemRow key={row.key} row={row} onSelect={() => runCommand(row.command)} />
          ))}
        </CommandPrimitive.List>
        <div className="pointer-events-none absolute inset-x-0 bottom-0 h-5 bg-gradient-to-b from-transparent to-popover" />
      </div>

      {/* Footer */}
      <div className="flex h-[34px] flex-none items-center gap-3.5 border-t border-border/60 bg-muted/30 px-3.5">
        <FooterHint keys="↑↓" label="navigate" />
        <FooterHint keys="↵" label="open" />
        <FooterHint keys={COMMAND_MODE_PREFIX} label="commands" />
        <span className="flex-1" />
        <span className="font-mono text-[10px] text-muted-foreground/70">
          {resultCountLabel(totalShown)}
        </span>
      </div>
    </CommandPrimitive>
  );
}

// ── Row primitives ───────────────────────────────────────────────────────

function GroupHeader({
  label,
  count,
  first,
}: {
  label: string;
  count: string;
  first?: boolean;
}) {
  return (
    <div
      className={cn(
        "sticky top-0 z-10 flex h-7 items-center gap-2.5 bg-popover px-2.5",
        !first && "mt-2",
      )}
    >
      <span className="font-mono text-[10px] tracking-[0.15em] text-muted-foreground/70 uppercase">
        {label}
      </span>
      <span className="h-px flex-1 bg-border/60" />
      <span className="font-mono text-[10px] text-muted-foreground/50">{count}</span>
    </div>
  );
}

/** Shared row chrome: the 40px selectable line every group renders. */
function PaletteItem({
  value,
  onSelect,
  children,
}: {
  value: string;
  onSelect: () => void;
  children: React.ReactNode;
}) {
  return (
    <CommandPrimitive.Item
      value={value}
      onSelect={onSelect}
      className="group/pal-row flex h-10 cursor-pointer items-center gap-2.5 rounded-lg px-2.5 outline-none select-none data-selected:bg-accent"
    >
      {children}
    </CommandPrimitive.Item>
  );
}

/** The "press enter" affordance that replaces a row's keybind hint while the
 *  row is selected — the palette's one moving part. */
function EnterBadge() {
  return (
    <span className="hidden flex-none rounded-[5px] border border-accent-ember/30 bg-accent-ember/10 px-1.5 py-0.5 font-mono text-[10px] text-accent-ember group-data-selected/pal-row:inline-flex">
      ↵
    </span>
  );
}

/** Avatars need a per-project hook, so each row owns its own component rather
 *  than the parent looping hooks (same pattern as the inbox rail). */
function RowAvatar({ name, path }: { name: string; path: string }) {
  const appearance = useProjectAppearance(path);
  return (
    <ProjectAvatar
      name={name}
      color={appearance.customColor}
      imageUrl={appearance.imageUrl}
      cacheBust={appearance.imageVersion}
      size="md"
      shape="square"
      className="flex-none"
    />
  );
}

function WorkspaceItem({
  row,
  now,
  onSelect,
}: {
  row: WorkspaceRow;
  now: number;
  onSelect: () => void;
}) {
  // Formatted here rather than baked into the row so the coarse clock can't
  // invalidate the whole ranked list on every render.
  const idleFor =
    row.status || row.activityAt === undefined
      ? null
      : formatElapsed(Math.max(0, now - row.activityAt));
  return (
    <PaletteItem value={row.key} onSelect={onSelect}>
      <RowAvatar name={row.projectName} path={row.projectPath} />
      <span className="flex min-w-0 flex-1 items-baseline gap-2.5">
        <span
          className={cn(
            "truncate text-[13px] font-medium",
            row.parked ? "text-muted-foreground" : "text-foreground/90",
          )}
        >
          {row.workspace.title}
        </span>
        <span className="max-w-[220px] flex-none truncate font-mono text-[11px] text-muted-foreground/70">
          {workspaceRowSubtitle(row.workspace, row.projectName)}
        </span>
      </span>
      {row.status && (
        <span
          className={cn(
            "flex flex-none items-center gap-1.5 text-[11px]",
            STATUS_TEXT_CLASS[row.status],
          )}
        >
          <span className={cn("size-1.5 rounded-full", STATUS_DOT_CLASS[row.status])} />
          {STATUS_LABEL[row.status]}
        </span>
      )}
      {!row.status && idleFor && (
        <span className="flex-none font-mono text-[11px] text-muted-foreground/70">
          {idleFor}
        </span>
      )}
      <EnterBadge />
    </PaletteItem>
  );
}

function ProjectItem({
  row,
  homeDir,
  onSelect,
}: {
  row: ProjectRow;
  homeDir: string | null;
  onSelect: () => void;
}) {
  return (
    <PaletteItem value={row.key} onSelect={onSelect}>
      <RowAvatar name={row.group.projectName} path={row.group.projectPath} />
      <span className="flex min-w-0 flex-1 items-baseline gap-2.5">
        <span className="truncate text-[13px] font-medium text-foreground/90">
          {row.group.projectName}
        </span>
        <span className="flex-none truncate font-mono text-[11px] text-muted-foreground/70">
          {shortenPath(row.group.projectPath, homeDir)}
        </span>
      </span>
      <span className="flex-none font-mono text-[11px] text-muted-foreground/70">
        {row.activeCount > 0 ? `${row.activeCount} active` : "idle"}
      </span>
      <EnterBadge />
    </PaletteItem>
  );
}

function CommandItemRow({ row, onSelect }: { row: CommandRow; onSelect: () => void }) {
  const Icon = row.command.icon;
  return (
    <PaletteItem value={row.key} onSelect={onSelect}>
      <span className="flex size-5 flex-none items-center justify-center rounded border border-border/60 text-muted-foreground/70">
        <Icon className="size-[11px]" />
      </span>
      <span className="min-w-0 flex-1 truncate text-[13px] font-medium text-foreground/90">
        {row.command.label}
      </span>
      {row.keys && (
        <span className="flex-none rounded-[5px] border border-border/60 px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground group-data-selected/pal-row:hidden">
          {row.keys}
        </span>
      )}
      <EnterBadge />
    </PaletteItem>
  );
}

function FooterHint({ keys, label }: { keys: string; label: string }) {
  return (
    <span className="flex items-center gap-1.5 text-[11px] text-muted-foreground/70">
      <kbd className="rounded border border-border/60 px-1.5 py-px font-mono text-[10px] text-muted-foreground">
        {keys}
      </kbd>
      {label}
    </span>
  );
}
