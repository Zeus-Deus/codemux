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
  /** Settled or snoozed workspaces, folded behind "Show N settled". Flat:
   *  a parked child has no live parent row to hang from. */
  parked: WorkspaceSnapshot[];
  /** Live workspaces in this section, children included — the header count. */
  liveCount: number;
}

export interface ProjectsModel {
  /** Workspaces that belong to no git project: home-directory chats and
   *  folders that were never a project. Null when there are none. */
  chats: ProjectsSection | null;
  projects: ProjectsSection[];
}

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
 *  net the inbox applies), without writing to the lifecycle store. */
export function buildProjectsModel(input: {
  projectGroups: readonly ProjectGroup[];
  homeDir: string | null;
  storedIndex: ReadonlyMap<string, number>;
  parkedIds: ReadonlySet<string>;
  liveOverride?: ReadonlySet<string>;
  /** Inbox project filter (a project path), or null for all projects. */
  filter: string | null;
}): ProjectsModel {
  const { projectGroups, homeDir, storedIndex, parkedIds, filter } = input;
  const liveOverride = input.liveOverride ?? new Set<string>();
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
    return { name, path, rows, parked, liveCount: countRows(rows) };
  };

  const chatWorkspaces: WorkspaceSnapshot[] = [];
  const projects: ProjectsSection[] = [];
  for (const group of projectGroups) {
    if (filter !== null && group.projectPath !== filter) continue;
    const own: WorkspaceSnapshot[] = [];
    for (const ws of group.workspaces) {
      if (isChatWorkspace(ws, homeDir)) chatWorkspaces.push(ws);
      else own.push(ws);
    }
    if (own.length > 0) {
      projects.push(section(group.projectName, group.projectPath, own));
    }
  }

  return {
    chats:
      chatWorkspaces.length > 0
        ? section("Chats", homeDir ?? "", chatWorkspaces)
        : null,
    projects,
  };
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
