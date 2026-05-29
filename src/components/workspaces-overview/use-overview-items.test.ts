/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { renderHook, cleanup } from "@testing-library/react";

import type { WorkspaceSnapshot } from "@/tauri/types";
import type { HostView, WorkspaceSyncView } from "@/tauri/commands";

// ── Mutable mock state ────────────────────────────────────────────

let mockWorkspaces: WorkspaceSnapshot[] | null = [];
let mockHosts: HostView[] = [];
let mockSyncRows: WorkspaceSyncView[] = [];

vi.mock("@/stores/app-store", () => ({
  useAppStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({
      appState: mockWorkspaces
        ? { workspaces: mockWorkspaces, active_workspace_id: null }
        : null,
    }),
  ),
  useHomeDir: () => "/home/test",
  // The hook calls `groupWorkspacesByProject` purely for the
  // workspace_id → projectName mapping. A minimal stub that buckets
  // by project_root (or cwd) is sufficient — the real impl is
  // exercised in app-store tests.
  groupWorkspacesByProject: (workspaces: WorkspaceSnapshot[]) => {
    const map = new Map<string, WorkspaceSnapshot[]>();
    for (const ws of workspaces) {
      const path = ws.project_root ?? ws.cwd;
      if (!map.has(path)) map.set(path, []);
      map.get(path)!.push(ws);
    }
    return Array.from(map.entries()).map(([path, list]) => ({
      projectPath: path,
      projectName: path.split("/").pop() ?? path,
      workspaces: list,
    }));
  },
}));

vi.mock("@/stores/hosts-store", () => ({
  useHosts: () => mockHosts,
}));

vi.mock("@/stores/workspaces-sync-store", () => ({
  useWorkspacesSync: () => mockSyncRows,
}));

import { useOverviewItems, remoteProjectName } from "./use-overview-items";

// ── Factories ─────────────────────────────────────────────────────

function makeWorkspace(
  partial: Partial<WorkspaceSnapshot> & { workspace_id: string },
): WorkspaceSnapshot {
  return {
    workspace_id: partial.workspace_id,
    title: partial.title ?? partial.workspace_id,
    workspace_type: partial.workspace_type ?? "standard",
    cwd: partial.cwd ?? "/home/test/proj",
    git_branch: partial.git_branch ?? null,
    git_ahead: partial.git_ahead ?? 0,
    git_behind: partial.git_behind ?? 0,
    git_additions: partial.git_additions ?? 0,
    git_deletions: partial.git_deletions ?? 0,
    git_changed_files: partial.git_changed_files ?? 0,
    notification_count: partial.notification_count ?? 0,
    latest_agent_state: partial.latest_agent_state ?? null,
    worktree_path: partial.worktree_path ?? null,
    project_root: partial.project_root ?? "/home/test/proj",
    pr_number: partial.pr_number ?? null,
    pr_state: partial.pr_state ?? null,
    pr_url: partial.pr_url ?? null,
    linked_issue: partial.linked_issue ?? null,
    notifications_muted: partial.notifications_muted ?? false,
    tabs: partial.tabs ?? [],
    active_tab_id: partial.active_tab_id ?? "",
    active_surface_id: partial.active_surface_id ?? "",
    surfaces: partial.surfaces ?? [],
    host_id: partial.host_id,
  } as WorkspaceSnapshot;
}

function makeSync(
  partial: Partial<WorkspaceSyncView> & { id: number },
): WorkspaceSyncView {
  return {
    id: partial.id,
    server_id: partial.server_id ?? `srv-${partial.id}`,
    workspace_id: partial.workspace_id ?? null,
    title: partial.title ?? `sync-${partial.id}`,
    host_server_id: partial.host_server_id ?? null,
    // Honor an explicit `null` (the "host never recorded a project
    // root" case) — `??` alone would silently swap it for the default.
    project_path:
      "project_path" in partial
        ? partial.project_path ?? null
        : "/home/test/proj",
    project_remote: partial.project_remote ?? null,
    git_branch: partial.git_branch ?? null,
    git_head_sha: partial.git_head_sha ?? null,
    created_at: partial.created_at ?? "2026-01-01T00:00:00Z",
    updated_at: partial.updated_at ?? "2026-01-01T00:00:00Z",
    dirty: partial.dirty ?? false,
  };
}

function makeHost(
  id: number,
  name: string,
  serverId: string | null = `srv-host-${id}`,
): HostView {
  return {
    id,
    server_id: serverId,
    name,
    ssh_target: `${name}@example`,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    dirty: false,
  };
}

afterEach(() => cleanup());

describe("useOverviewItems", () => {
  beforeEach(() => {
    mockWorkspaces = [];
    mockHosts = [];
    mockSyncRows = [];
  });

  it("renders a local workspace as kind:local and consumes its sync row", () => {
    mockWorkspaces = [
      makeWorkspace({ workspace_id: "ws-1", title: "alpha" }),
    ];
    mockSyncRows = [makeSync({ id: 10, workspace_id: "ws-1" })];

    const { result } = renderHook(() => useOverviewItems());

    expect(result.current.items).toHaveLength(1);
    expect(result.current.items[0]?.kind).toBe("local");
    if (result.current.items[0]?.kind === "local") {
      expect(result.current.items[0].workspace.workspace_id).toBe("ws-1");
      expect(result.current.items[0].sync?.id).toBe(10);
    }
  });

  it("renders a sync row whose workspace_id is null as kind:remote (sibling-device)", () => {
    mockSyncRows = [
      makeSync({
        id: 11,
        workspace_id: null,
        title: "lives on devbox",
        host_server_id: "srv-host-7",
      }),
    ];

    const { result } = renderHook(() => useOverviewItems());

    expect(result.current.items).toHaveLength(1);
    expect(result.current.items[0]?.kind).toBe("remote");
    if (result.current.items[0]?.kind === "remote") {
      expect(result.current.items[0].sync.title).toBe("lives on devbox");
    }
  });

  it("DOES NOT render a sync row whose workspace_id is set but app_state no longer has it", () => {
    // Regression guard for the "closed workspace briefly appears as
    // 'lives on another device'" bug. When the close path's reconcile
    // hasn't pruned the `workspaces_sync` row yet (or the row is stale
    // for any other reason), `workspace_id` is still set to a local id
    // that no longer exists in `app_state.workspaces`. This is a
    // self-orphan — NOT a sibling-device workspace. The overview must
    // skip it rather than tagging it as "other device".
    mockWorkspaces = []; // workspace "ws-closed" has been closed
    mockSyncRows = [
      makeSync({
        id: 12,
        workspace_id: "ws-closed",
        title: "just-closed",
        host_server_id: null,
      }),
    ];

    const { result } = renderHook(() => useOverviewItems());

    expect(result.current.items).toHaveLength(0);
  });

  it("self-orphan filter does not hide true sibling-device rows in mixed state", () => {
    // Mix: one live local workspace, one orphan from a recent close,
    // one true sibling-device row. Only the local and the sibling
    // should appear.
    mockWorkspaces = [
      makeWorkspace({ workspace_id: "ws-live", title: "still-here" }),
    ];
    mockSyncRows = [
      makeSync({ id: 20, workspace_id: "ws-live", title: "still-here" }),
      makeSync({
        id: 21,
        workspace_id: "ws-closed",
        title: "stale-orphan",
      }),
      makeSync({
        id: 22,
        workspace_id: null,
        title: "on-sibling",
        host_server_id: "srv-host-7",
      }),
    ];

    const { result } = renderHook(() => useOverviewItems());

    const kinds = result.current.items.map((i) => i.kind);
    const titles = result.current.items.map((i) =>
      i.kind === "local" ? i.workspace.title : i.sync.title,
    );

    expect(result.current.items).toHaveLength(2);
    expect(kinds).toContain("local");
    expect(kinds).toContain("remote");
    expect(titles).toContain("still-here");
    expect(titles).toContain("on-sibling");
    expect(titles).not.toContain("stale-orphan");
  });

  it("falls back to the sync title for a remote row whose project_path is null", () => {
    // A root/main checkout an agent created via the MCP `workspace_create`
    // tool reaches us with project_path=null (the host never recorded a
    // project_root for a non-worktree). The overview must not render a
    // bare "—" — the workspace title is the meaningful fallback.
    mockSyncRows = [
      makeSync({
        id: 30,
        workspace_id: null,
        title: "passpage",
        project_path: null,
        host_server_id: "srv-host-7",
      }),
    ];

    const { result } = renderHook(() => useOverviewItems());

    expect(result.current.items[0]?.projectName).toBe("passpage");
  });

  it("does not duplicate-render: title fallback only kicks in without a project_path basename", () => {
    mockSyncRows = [
      makeSync({
        id: 31,
        workspace_id: null,
        title: "passpage-ui-polish",
        project_path: "/home/agent/projects/passpage",
        host_server_id: "srv-host-7",
      }),
    ];

    const { result } = renderHook(() => useOverviewItems());

    // project_path basename wins over the title when present.
    expect(result.current.items[0]?.projectName).toBe("passpage");
  });

  it("resolves hostServerId on a local item via the hosts table when no sync row exists yet", () => {
    mockHosts = [makeHost(7, "devbox", "srv-host-7")];
    mockWorkspaces = [
      makeWorkspace({
        workspace_id: "ws-remote",
        title: "on-devbox",
        host_id: 7,
      }),
    ];
    mockSyncRows = []; // first reconcile hasn't run yet

    const { result } = renderHook(() => useOverviewItems());

    expect(result.current.items).toHaveLength(1);
    if (result.current.items[0]?.kind === "local") {
      expect(result.current.items[0].hostServerId).toBe("srv-host-7");
    }
  });
});

describe("remoteProjectName", () => {
  it("prefers the project_path basename", () => {
    expect(
      remoteProjectName(
        makeSync({
          id: 1,
          title: "wt-branch",
          project_path: "/home/agent/projects/passpage",
        }),
      ),
    ).toBe("passpage");
  });

  it("falls back to the title when project_path is null", () => {
    expect(
      remoteProjectName(
        makeSync({ id: 2, title: "passpage", project_path: null }),
      ),
    ).toBe("passpage");
  });

  it("returns null only when there is no path basename and no title", () => {
    expect(
      remoteProjectName(makeSync({ id: 3, title: "", project_path: null })),
    ).toBeNull();
  });
});
