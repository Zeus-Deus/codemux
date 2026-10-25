import type { ProjectGroup } from "@/stores/app-store";
import type { WorkspaceSnapshot } from "@/tauri/types";

/** One workspace row in the Projects view, with the follow-up children that
 *  nest under it (indented, behind a guide line). */
export interface ProjectsRow {
  workspace: WorkspaceSnapshot;
  children: ProjectsRow[];
}

export interface ProjectsSection {
  name: string;
  path: string;
  /** Live rows, top level in newest-first order, children nested. */
  rows: ProjectsRow[];
  /** Settled or snoozed workspaces, newest first. Flat: a parked child has
   *  no live parent row to hang from. Listed in the shelf, and under the
   *  header of a project with nothing live. */
  parked: WorkspaceSnapshot[];
  /** Live workspaces in this section, children included — the header count. */
  liveCount: number;
  /** Newest agent activity or visit across the section's workspaces (ms
   *  epoch), or null when none is known. */
  lastActiveAt: number | null;
}

/** One parked workspace in the sidebar-wide Settled shelf. */
export interface ShelfEntry {
  workspace: WorkspaceSnapshot;
  sectionName: string;
  sectionPath: string;
}

export interface ProjectsModel {
  /** Live home-directory chats and folders that were never a project. Null
   *  when nothing there is live; parked chats live in the shelf. */
  chats: ProjectsSection | null;
  /** Projects with live work, recent activity, or the open workspace. */
  projects: ProjectsSection[];
  /** Everything else, folded behind "N more projects". */
  quietProjects: ProjectsSection[];
  /** Every parked workspace, most recently parked first. */
  shelf: ShelfEntry[];
}

/** How long a project with nothing live stays in the main list after its
 *  last activity, so settling the last chat doesn't make it vanish at once. */
export const RECENT_PROJECT_MS = 3 * 24 * 60 * 60 * 1000;

/** Whether a workspace is a plain chat rather than work inside a project.
 *
 *  Uses the same identity the rest of the sidebar groups by
 *  (`project_root || cwd`): anything rooted at the home directory is the
 *  "Home" group everywhere else, and a Home-type workspace is always a chat.
 *  A workspace with no project root that is not a git checkout is a chat
 *  opened in a loose folder. A non-git folder that *was* opened as a project
 *  (`project_root` set) stays a project — the user chose to treat it as one. */
export function isChatWorkspace(
  ws: WorkspaceSnapshot,
  homeDir: string | null,
): boolean {
  if (ws.workspace_type === "home") return true;
  const root = ws.project_root || ws.cwd;
  if (homeDir !== null && root === homeDir) return true;
  return !ws.project_root && ws.is_git === false;
}

/** Nest follow-up children under their parent.
 *
 *  `ordered` is the section's live list in display order. A child renders
 *  under its parent only when that parent is itself a live row of the same
 *  section; a parent that is missing, parked, or in another section leaves
 *  the child at top level as a normal row. A parent chain that loops (bad
 *  data) is broken by treating the looping row as top level, so no row can
 *  ever disappear. Children keep the section's order. */
export function nestRows(ordered: readonly WorkspaceSnapshot[]): ProjectsRow[] {
  const byId = new Map<string, WorkspaceSnapshot>();
  for (const ws of ordered) byId.set(ws.workspace_id, ws);

  // Resolve each row's effective parent: present in this list and reachable
  // without a cycle.
  const parentOf = new Map<string, string>();
  for (const ws of ordered) {
    const parentId = ws.parent_workspace_id;
    if (!parentId || parentId === ws.workspace_id || !byId.has(parentId)) {
      continue;
    }
    const seen = new Set<string>([ws.workspace_id]);
    let cursor: string | null | undefined = parentId;
    let cycle = false;
    while (cursor) {
      if (seen.has(cursor)) {
        cycle = true;
        break;
      }
      seen.add(cursor);
      const next: string | null | undefined =
        byId.get(cursor)?.parent_workspace_id;
      cursor = next && byId.has(next) ? next : null;
    }
    if (!cycle) parentOf.set(ws.workspace_id, parentId);
  }

  const nodes = new Map<string, ProjectsRow>();
  for (const ws of ordered) {
    nodes.set(ws.workspace_id, { workspace: ws, children: [] });
  }
  const roots: ProjectsRow[] = [];
  for (const ws of ordered) {
    const node = nodes.get(ws.workspace_id)!;
    const parentId = parentOf.get(ws.workspace_id);
    if (parentId) nodes.get(parentId)!.children.push(node);
    else roots.push(node);
  }
  return roots;
}

function countRows(rows: readonly ProjectsRow[]): number {
  let n = 0;
  for (const row of rows) n += 1 + countRows(row.children);
  return n;
}

/** Pure Projects-view grouping, memoized by the caller.
 *
 *  `projectGroups` is the app-wide grouping (`groupWorkspacesByProject`), so
 *  project names, Home labeling and host suffixes match every other surface.
 *  `storedIndex` gives the static newest-first order the inbox uses: it is
 *  creation order, never status, so rows hold still while agents run.
 *  `parkedIds` are settled/snoozed workspaces; `liveOverride` lets a parked
 *  workspace whose agent is working or blocked stay visible (the same safety
 *  net the inbox applies), without writing to the lifecycle store.
 *
 *  A project stays in the main list while it has live work, holds the open
 *  workspace, or saw activity within `RECENT_PROJECT_MS`; the rest go to
 *  `quietProjects`. A filtered view always shows its one project. */
export function buildProjectsModel(input: {
  projectGroups: readonly ProjectGroup[];
  homeDir: string | null;
  storedIndex: ReadonlyMap<string, number>;
  parkedIds: ReadonlySet<string>;
  liveOverride?: ReadonlySet<string>;
  /** Inbox project filter (a project path), or null for all projects. */
  filter: string | null;
  /** When each parked workspace was parked (ms epoch) — the shelf order. */
  parkedAt?: ReadonlyMap<string, number>;
  /** The open workspace: its section always stays in the main list. */
  activeWorkspaceId?: string;
  /** Project paths that must stay in the main list (a workspace is being
   *  created there). */
  forcePaths?: ReadonlySet<string>;
  /** How long a project with nothing live stays in the main list; null keeps
   *  every project there. Defaults to `RECENT_PROJECT_MS`. A plugin's
   *  declared sidebar view sets it. */
  quietAfterMs?: number | null;
  /** Projects with no workspaces that still get a heading (a declared view
   *  that shows empty projects). Ones already grouped are ignored. */
  emptyProjects?: readonly { name: string; path: string }[];
  now: number;
}): ProjectsModel {
  const { projectGroups, homeDir, storedIndex, parkedIds, filter, now } = input;
  const liveOverride = input.liveOverride ?? new Set<string>();
  const parkedAt = input.parkedAt ?? new Map<string, number>();
  const forcePaths = input.forcePaths ?? new Set<string>();
  const quietAfterMs = input.quietAfterMs === undefined ? RECENT_PROJECT_MS : input.quietAfterMs;
  const newestFirst = (a: WorkspaceSnapshot, b: WorkspaceSnapshot) =>
    (storedIndex.get(b.workspace_id) ?? 0) -
    (storedIndex.get(a.workspace_id) ?? 0);

  const isParked = (ws: WorkspaceSnapshot) =>
    ws.pinned_at == null &&
    parkedIds.has(ws.workspace_id) &&
    !liveOverride.has(ws.workspace_id);

  const section = (
    name: string,
    path: string,
    workspaces: WorkspaceSnapshot[],
  ): ProjectsSection => {
    const live: WorkspaceSnapshot[] = [];
    const parked: WorkspaceSnapshot[] = [];
    for (const ws of [...workspaces].sort(newestFirst)) {
      (isParked(ws) ? parked : live).push(ws);
    }
    const rows = nestRows(live);
    let lastActiveAt: number | null = null;
    for (const ws of workspaces) {
      for (const at of [ws.last_active_at, ws.last_visited_at]) {
        if (at != null && (lastActiveAt === null || at > lastActiveAt)) {
          lastActiveAt = at;
        }
      }
    }
    return { name, path, rows, parked, liveCount: countRows(rows), lastActiveAt };
  };

  const holdsActive = (s: ProjectsSection) =>
    input.activeWorkspaceId !== undefined &&
    s.parked.some((ws) => ws.workspace_id === input.activeWorkspaceId);

  const shelf: ShelfEntry[] = [];
  const shelve = (s: ProjectsSection) => {
    for (const ws of s.parked) {
      shelf.push({ workspace: ws, sectionName: s.name, sectionPath: s.path });
    }
  };

  const chatWorkspaces: WorkspaceSnapshot[] = [];
  const projects: ProjectsSection[] = [];
  const quietProjects: ProjectsSection[] = [];
  for (const group of projectGroups) {
    if (filter !== null && group.projectPath !== filter) continue;
    const own: WorkspaceSnapshot[] = [];
    for (const ws of group.workspaces) {
      if (isChatWorkspace(ws, homeDir)) chatWorkspaces.push(ws);
      else own.push(ws);
    }
    if (own.length === 0) continue;
    const s = section(group.projectName, group.projectPath, own);
    shelve(s);
    const shown =
      filter !== null ||
      s.rows.length > 0 ||
      holdsActive(s) ||
      forcePaths.has(s.path) ||
      quietAfterMs === null ||
      (s.lastActiveAt !== null && now - s.lastActiveAt < quietAfterMs);
    (shown ? projects : quietProjects).push(s);
  }
  if (filter === null) {
    const grouped = new Set(projectGroups.map((group) => group.projectPath));
    for (const empty of input.emptyProjects ?? []) {
      if (grouped.has(empty.path) || empty.path === homeDir) continue;
      const s = section(empty.name, empty.path, []);
      // Nothing dates an empty project, so only a view that never folds keeps it up.
      (quietAfterMs === null || forcePaths.has(s.path) ? projects : quietProjects).push(s);
    }
  }

  let chats: ProjectsSection | null = null;
  if (chatWorkspaces.length > 0) {
    const s = section("Chats", homeDir ?? "", chatWorkspaces);
    shelve(s);
    if (s.rows.length > 0 || holdsActive(s)) chats = s;
  }

  // Stable sort: equal timestamps keep section order.
  shelf.sort(
    (a, b) =>
      (parkedAt.get(b.workspace.workspace_id) ?? 0) -
      (parkedAt.get(a.workspace.workspace_id) ?? 0),
  );

  return { chats, projects, quietProjects, shelf };
}

/** The shelf's first `limit` entries grouped under their section, sections in
 *  order of their most recent entry. `keepId` (the open workspace) is pulled
 *  in from past the limit so it never hides behind "Show more". */
export function groupShelf(
  shelf: readonly ShelfEntry[],
  limit: number,
  keepId?: string,
): { name: string; path: string; workspaces: WorkspaceSnapshot[] }[] {
  const visible = shelf.slice(0, limit);
  const kept = shelf
    .slice(limit)
    .find((e) => e.workspace.workspace_id === keepId);
  if (kept) visible.push(kept);
  const groups = new Map<
    string,
    { name: string; path: string; workspaces: WorkspaceSnapshot[] }
  >();
  for (const e of visible) {
    let g = groups.get(e.sectionPath);
    if (!g) {
      g = { name: e.sectionName, path: e.sectionPath, workspaces: [] };
      groups.set(e.sectionPath, g);
    }
    g.workspaces.push(e.workspace);
  }
  return [...groups.values()];
}

/** Live row ids in visual order, honouring folded groups — the Alt+1..9
 *  jump targets for this view. */
export function visibleLiveIds(
  model: ProjectsModel,
  folded: Readonly<Record<string, true>>,
): string[] {
  const out: string[] = [];
  const walk = (rows: readonly ProjectsRow[]) => {
    for (const row of rows) {
      out.push(row.workspace.workspace_id);
      walk(row.children);
    }
  };
  if (model.chats) walk(model.chats.rows);
  for (const project of model.projects) {
    if (!folded[project.path]) walk(project.rows);
  }
  return out;
}
