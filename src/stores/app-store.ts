import { useMemo } from "react";
import { create } from "zustand";
import type {
  AppStateSnapshot,
  PaneNodeSnapshot,
  WorkspaceSnapshot,
  SurfaceSnapshot,
} from "@/tauri/types";
import { basename, tailSegments, segmentCount } from "@/lib/path";
import { shareStructural } from "@/lib/structural-share";
import type { HostView } from "@/tauri/commands";

interface AppStore {
  appState: AppStateSnapshot | null;
  /** Cached `$HOME` string from the `get_home_dir` Tauri call. Null
   *  until hydrated at App mount. Consumers (project-group selector,
   *  lazy-draft home detection) should treat null as "not yet known"
   *  and fall back to today's path-basename grouping. */
  homeDir: string | null;
  /** Workspace id currently being pushed to or pulled from a remote
   *  host. Drives the spinner icon on the sidebar row so the user
   *  sees the operation is in flight. Null when no push/pull is
   *  running. Set by the workspace context menu's Move/Pull handlers
   *  and cleared in the completion callback (success or failure). */
  workspacePushPullInFlight: string | null;
  /** Timestamp (Date.now()) when the current push/pull started. Used
   *  by the overview row to render an "elapsed Ns" label when an
   *  operation takes longer than ~2 seconds (Phase-4d signal). Null
   *  when no operation is in flight. */
  workspacePushPullStartedAt: number | null;
  setAppState: (snapshot: AppStateSnapshot) => void;
  setHomeDir: (homeDir: string) => void;
  setWorkspacePushPullInFlight: (workspaceId: string | null) => void;
}

export const useAppStore = create<AppStore>((set) => ({
  appState: null,
  homeDir: null,
  workspacePushPullInFlight: null,
  workspacePushPullStartedAt: null,
  // Structural sharing: reconcile the incoming snapshot against the one we
  // already hold so unchanged subtrees keep their previous reference. When
  // nothing changed the returned ref equals the previous snapshot, so zustand
  // selector subscribers see zero fan-out. See `@/lib/structural-share`.
  setAppState: (snapshot) =>
    set((state) => ({
      appState: state.appState
        ? shareStructural(state.appState, snapshot)
        : snapshot,
    })),
  setHomeDir: (homeDir) => set({ homeDir }),
  // Pair the workspace id with a start timestamp so "elapsed" can
  // be computed at render time. Null clears both.
  setWorkspacePushPullInFlight: (workspaceId) =>
    set({
      workspacePushPullInFlight: workspaceId,
      workspacePushPullStartedAt: workspaceId === null ? null : Date.now(),
    }),
}));

// Derived selectors
//
// IMPORTANT — re-render economics:
//
// The Rust backend rebuilds the full `AppStateSnapshot` (workspaces,
// surfaces, panes, tabs) on every `emit_app_state` and ships it to the
// renderer over Tauri IPC. Every workspace object — and every nested
// surfaces/tabs array — therefore has a *fresh reference* on every
// backend tick. Agent token streaming, the 5-second git poll, hook
// events, and PR/port refreshes all fire emit_app_state, so under
// normal use these ticks happen many times per second.
//
// `useActiveWorkspace()` returns the matching workspace OBJECT from the
// snapshot. Before structural sharing, a fresh snapshot meant a fresh
// workspace reference every tick, so every consumer subscribed via this
// hook re-rendered on every tick — cascading into MarkdownRendered (full
// markdown re-parse), the pane container, the tab bar, etc.
//
// Mitigation:
//
// 0. `setAppState` (above) now runs `shareStructural(prev, next)` before
//    storing the snapshot. It reconciles the fresh IPC payload against the
//    snapshot we already hold and REUSES prev references for every subtree
//    that is deep-equal — so a no-op backend tick returns the *same*
//    top-level ref (zero selector fan-out), and a change to one workspace
//    leaves all other workspaces', surfaces', and pane subtrees' refs
//    untouched. This is what makes `useActiveWorkspace` / `useActiveSurface`
//    stable across no-op ticks, and it is what makes the `React.memo`
//    boundaries on the pane tree (added in the same change) actually pay off
//    — an unchanged pane subtree keeps its identity, so memo short-circuits.
//    (This is NOT the old full-snapshot JSON.stringify dedup, which was
//    removed for causing freezes — see `src/hooks/use-app-state.ts`.)
//
// 1. Primitive-slice selectors (`useActiveWorkspaceId`,
//    `useActiveWorkspaceCwd`, etc.) below return primitive strings
//    that compare with `===` and are stable across backend ticks
//    unless the slice itself changed. Lightweight consumers (title-bar
//    SearchTrigger, run-button, file-search dialog, content-search
//    dialog) use these instead of the full-object selector.
//
// 2. `useActiveWorkspace()` is kept for consumers that legitimately
//    need the full workspace object (WorkspaceMain). The heaviest cost
//    downstream of WorkspaceMain — react-markdown re-parsing — is
//    mitigated separately by `React.memo` on `MarkdownRendered`.
//
// Before structural sharing, `useShallow` would NOT have helped: the Rust
// snapshot rebuild gave every nested array (surfaces, tabs) fresh refs, so
// a shallow compare always tripped. Structural sharing fixes this at the
// source by restoring reference stability for unchanged subtrees.
export function useActiveWorkspace(): WorkspaceSnapshot | null {
  return useAppStore((s) => {
    if (!s.appState) return null;
    return (
      s.appState.workspaces.find(
        (w) => w.workspace_id === s.appState!.active_workspace_id,
      ) ?? null
    );
  });
}

/** Primitive: the active workspace's id, or null when no workspace is
 *  active. Stable across backend ticks unless the active workspace
 *  actually changes. Use this in components that only need the id. */
export function useActiveWorkspaceId(): string | null {
  return useAppStore((s) => s.appState?.active_workspace_id ?? null);
}

/** Primitive: the active workspace's cwd, or null. Used by search
 *  dialogs that need a path, not the whole workspace. Returns the
 *  primitive string so subscribers don't churn on full-snapshot
 *  rebuilds. */
export function useActiveWorkspaceCwd(): string | null {
  return useAppStore((s) => {
    const id = s.appState?.active_workspace_id;
    if (!id) return null;
    const ws = s.appState!.workspaces.find((w) => w.workspace_id === id);
    return ws?.cwd ?? null;
  });
}

/** Primitive: the active workspace's project_root, or null. */
export function useActiveWorkspaceProjectRoot(): string | null {
  return useAppStore((s) => {
    const id = s.appState?.active_workspace_id;
    if (!id) return null;
    const ws = s.appState!.workspaces.find((w) => w.workspace_id === id);
    return ws?.project_root ?? null;
  });
}

/** Primitive: the active workspace's git branch, or null. Used for
 *  the search-trigger label in the title bar. */
export function useActiveWorkspaceBranch(): string | null {
  return useAppStore((s) => {
    const id = s.appState?.active_workspace_id;
    if (!id) return null;
    const ws = s.appState!.workspaces.find((w) => w.workspace_id === id);
    return ws?.git_branch ?? null;
  });
}

/** Subscribe to the cached home directory. Null until the initial
 *  `getHomeDir()` Tauri call resolves at App mount. */
export function useHomeDir(): string | null {
  return useAppStore((s) => s.homeDir);
}

/** Recursively walk a pane tree checking whether the given pane id is
 *  present anywhere under the node. */
function paneTreeContains(node: PaneNodeSnapshot, paneId: string): boolean {
  if (node.pane_id === paneId) return true;
  if (node.kind === "split") {
    return node.children.some((child) => paneTreeContains(child, paneId));
  }
  return false;
}

/** Recursively collect every terminal pane's `session_id` under the given
 *  node into the supplied workspace map. Mutates `out` in place to avoid
 *  intermediate Map allocations during the walk. */
function collectTerminalSessions(
  node: PaneNodeSnapshot,
  workspaceId: string,
  out: Map<string, string>,
): void {
  if (node.kind === "terminal") {
    out.set(node.session_id, workspaceId);
    return;
  }
  if (node.kind === "split") {
    for (const child of node.children) {
      collectTerminalSessions(child, workspaceId, out);
    }
  }
  // browser / agent_chat panes have no session_id — skip.
}

/** Build a reverse index from `session_id` to the `workspace_id` that owns
 *  the terminal pane carrying that session.
 *
 *  Replaces the O(panes^2) `JSON.stringify(surf.root).includes(sid)` scan
 *  used by the terminal scrollback save / restore paths. With many
 *  workspaces, that pattern stringified every surface tree once per
 *  terminal mount and once per scrollback save — wasteful at scale. The
 *  index is built once per `AppStateSnapshot` reference (see
 *  `getSessionWorkspaceId` for caching) so repeated lookups are O(1).
 *
 *  Pure function of the snapshot — exported for unit testing.
 */
export function buildSessionWorkspaceIndex(
  appState: AppStateSnapshot | null,
): Map<string, string> {
  const index = new Map<string, string>();
  if (!appState) return index;
  for (const ws of appState.workspaces) {
    for (const surface of ws.surfaces) {
      collectTerminalSessions(surface.root, ws.workspace_id, index);
    }
  }
  return index;
}

// WeakMap caches the index per snapshot reference. The Rust-side state
// rebuilds the snapshot on every change, so a new reference always means a
// stale index — and a re-used reference means the index is still correct.
const sessionWorkspaceIndexCache = new WeakMap<
  AppStateSnapshot,
  Map<string, string>
>();

function getCachedSessionWorkspaceIndex(
  appState: AppStateSnapshot | null,
): Map<string, string> {
  if (!appState) return new Map();
  const cached = sessionWorkspaceIndexCache.get(appState);
  if (cached) return cached;
  const fresh = buildSessionWorkspaceIndex(appState);
  sessionWorkspaceIndexCache.set(appState, fresh);
  return fresh;
}

/** Imperative lookup: workspace id for a given terminal session, or null
 *  if no terminal pane currently owns it. Reads the cached index from
 *  `useAppStore.getState()` so callers inside effects / async IIFEs do not
 *  pay the build cost on every call. Returns null (not "") so call sites
 *  can apply their own fallback. */
export function getSessionWorkspaceId(sessionId: string): string | null {
  const { appState } = useAppStore.getState();
  const index = getCachedSessionWorkspaceIndex(appState);
  return index.get(sessionId) ?? null;
}

/** React hook variant — subscribes to `appState` and returns the cached
 *  index. Re-renders only when the snapshot reference changes (which is
 *  also when the index would actually differ). */
export function useSessionWorkspaceIndex(): Map<string, string> {
  return useAppStore((s) => getCachedSessionWorkspaceIndex(s.appState));
}

/** Find the workspace id that owns a given pane, if any.
 *
 *  Used by `AgentChatPane`'s draft-aware mount guard: when the pane's
 *  own `thread_id` field hasn't arrived yet (the Stage C race between
 *  `agent_chat_start_session`'s state emit and the pane mount), we
 *  need to know which workspace the pane belongs to so we can look up
 *  the promoted draft. Pure function of the snapshot — safe to call
 *  from inside a Zustand selector.
 */
export function findWorkspaceIdForPane(
  state: { appState: AppStateSnapshot | null },
  paneId: string,
): string | null {
  if (!state.appState) return null;
  for (const ws of state.appState.workspaces) {
    for (const surface of ws.surfaces) {
      if (paneTreeContains(surface.root, paneId)) {
        return ws.workspace_id;
      }
    }
  }
  return null;
}

export function useActiveSurface(): SurfaceSnapshot | null {
  return useAppStore((s) => {
    if (!s.appState) return null;
    const ws = s.appState.workspaces.find(
      (w) => w.workspace_id === s.appState!.active_workspace_id,
    );
    if (!ws) return null;
    return ws.surfaces.find((sf) => sf.surface_id === ws.active_surface_id) ?? null;
  });
}

// Project grouping — groups unsorted workspaces by their project root directory

export interface ProjectGroup {
  projectName: string;
  projectPath: string;
  workspaces: WorkspaceSnapshot[];
}

/** Derive the project root for grouping: `project_root`, `cwd` fallback.
 *  Accepts anything carrying the two fields (live `WorkspaceSnapshot`s
 *  and `ArchivedWorkspaceSnapshot`s alike) so every surface groups by
 *  the same key. */
export function resolveProjectRoot(ws: {
  project_root?: string | null;
  cwd: string;
}): string {
  return ws.project_root || ws.cwd;
}

/**
 * The single label rule for a project root: the dedicated "Home" label
 * when the root IS the user's home directory, the path basename
 * otherwise. Shared by the sidebar grouping and Settings → Archive so
 * both surfaces name a project identically. (Duplicate-basename
 * disambiguation is a collision post-pass in `groupWorkspacesByProject`,
 * layered on top of this rule.)
 */
export function projectDisplayName(
  projectPath: string,
  homeDir: string | null,
): string {
  return homeDir !== null && projectPath === homeDir
    ? "Home"
    : basename(projectPath);
}

/**
 * Pure grouping — exported so unit tests can exercise the logic
 * without rendering a hook. `useProjectGroupedWorkspaces` is a thin
 * `useMemo` wrapper.
 *
 * When `homeDir` is non-null, workspaces whose resolved project root
 * equals `homeDir` land in a dedicated group labelled "Home" (instead
 * of e.g. "user" or "home/user" from the path-basename rule). When
 * `homeDir` is null, grouping falls back to today's path-only rule.
 *
 * Under the Step 5 Home rework (Stage B+), `sidebar-workspace-list`
 * stops filtering out `workspace_type === "home"` workspaces, so this
 * label rule is what surfaces them correctly in the sidebar.
 */
/**
 * The single host a group's workspaces live on, or `null` when they're
 * local. Returns `undefined` when the group is empty or its workspaces
 * straddle multiple hosts — in that case the host can't disambiguate
 * the group and we fall back to path tails.
 */
function groupHostId(workspaces: WorkspaceSnapshot[]): number | null | undefined {
  let hostId: number | null | undefined = undefined;
  for (const ws of workspaces) {
    const h = ws.host_id ?? null;
    if (hostId === undefined) hostId = h;
    else if (hostId !== h) return undefined; // mixed → ambiguous
  }
  return hostId;
}

export function groupWorkspacesByProject(
  workspaces: WorkspaceSnapshot[],
  homeDir: string | null,
  hostNameById?: ReadonlyMap<number, string> | null,
): ProjectGroup[] {
  const groups = new Map<string, { name: string; path: string; workspaces: WorkspaceSnapshot[] }>();

  for (const ws of workspaces) {
    const projectPath = resolveProjectRoot(ws);
    const projectName = projectDisplayName(projectPath, homeDir);

    if (!groups.has(projectPath)) {
      groups.set(projectPath, { name: projectName, path: projectPath, workspaces: [] });
    }
    groups.get(projectPath)!.workspaces.push(ws);
  }

  const result = Array.from(groups.values()).map((g) => ({
    projectName: g.name,
    projectPath: g.path,
    workspaces: g.workspaces,
  }));

  // Disambiguate duplicate project names. Two project roots can share a
  // basename when the same repo lives on different machines (e.g. a
  // local `~/projects/app` and the same `app` on a remote host) or in
  // sibling directories. We prefer the host as the distinguishing tag:
  // the local copy keeps its clean basename and each remote copy is
  // suffixed with " · <host>". When the host can't tell the copies
  // apart (both local, both on the same host, or host names
  // unavailable) we fall back to appending just enough trailing path
  // segments to make every colliding label unique. The "Home" group is
  // always unique (single homeDir path), so this never rewrites it.
  const byName = new Map<string, typeof result>();
  for (const g of result) {
    const list = byName.get(g.projectName);
    if (list) list.push(g);
    else byName.set(g.projectName, [g]);
  }

  for (const colliding of byName.values()) {
    if (colliding.length < 2) continue;

    // Pass 1 — tag remote groups with their host name; locals (and
    // groups whose host we can't name) keep the clean basename.
    for (const g of colliding) {
      const hostId = groupHostId(g.workspaces);
      const hostName = hostId != null ? hostNameById?.get(hostId) ?? null : null;
      if (hostName) g.projectName = `${g.projectName} · ${hostName}`;
    }

    const unique = (gs: typeof colliding) =>
      new Set(gs.map((g) => g.projectName)).size === gs.length;
    if (unique(colliding)) continue;

    // Pass 2 — fall back to trailing path segments, growing the tail
    // until every label is unique. Group keys are distinct absolute
    // paths, so this always converges by the longest path's depth.
    const maxSegments = Math.max(...colliding.map((g) => segmentCount(g.projectPath)));
    for (let n = 2; n <= maxSegments; n++) {
      const tails = colliding.map((g) => tailSegments(g.projectPath, n));
      if (new Set(tails).size === colliding.length) {
        colliding.forEach((g, i) => {
          if (tails[i].includes("/")) g.projectName = tails[i];
        });
        break;
      }
    }
  }

  return result;
}

export function useProjectGroupedWorkspaces(
  workspaces: WorkspaceSnapshot[],
  homeDir: string | null,
  hosts?: HostView[],
): ProjectGroup[] {
  return useMemo(() => {
    const hostNameById = hosts
      ? new Map(hosts.map((h) => [h.id, h.name] as const))
      : null;
    return groupWorkspacesByProject(workspaces, homeDir, hostNameById);
  }, [workspaces, homeDir, hosts]);
}
