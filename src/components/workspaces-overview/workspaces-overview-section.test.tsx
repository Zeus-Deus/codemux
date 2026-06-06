/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render } from "@testing-library/react";

import type { WorkspaceSnapshot } from "@/tauri/types";
import type { HostView } from "@/tauri/commands";

// ── Mutable mock state — exercised by individual tests ──

let mockWorkspaces: WorkspaceSnapshot[] | null = [];
let mockActiveWorkspaceId: string | null = null;
let mockHosts: HostView[] = [];
let mockHostsLoaded = true;

const mockInitHosts = vi.fn().mockResolvedValue(undefined);

vi.mock("@/stores/app-store", () => ({
  useAppStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({
      appState: mockWorkspaces
        ? {
            workspaces: mockWorkspaces,
            active_workspace_id: mockActiveWorkspaceId,
          }
        : null,
      // Used by the row component
      workspacePushPullInFlight: null,
      setWorkspacePushPullInFlight: vi.fn(),
    }),
  ),
  // groupWorkspacesByProject is reused by the section. Provide a
  // simple stub that puts every workspace into one bucket named by
  // its project_root basename. Sufficient for the bucket assertions
  // below — the real implementation is exercised in app-store tests.
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
  useHomeDir: () => "/home/test",
}));

vi.mock("@/stores/hosts-store", () => ({
  useHostsStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({
      hosts: mockHosts,
      loaded: mockHostsLoaded,
      init: mockInitHosts,
    }),
  ),
  useHosts: () => mockHosts,
}));

vi.mock("@/stores/workspaces-sync-store", () => ({
  useWorkspacesSync: () => [] as never[],
  useWorkspacesSyncStatus: () => ({
    rows: [],
    loading: false,
    loaded: true,
    error: null,
    refresh: () => Promise.resolve(),
  }),
  // The pull-to-device dialog (mounted by the section) reads
  // `refresh` from the underlying store to nudge a sync after a
  // successful adoption.
  useWorkspacesSyncStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({
      rows: [],
      loading: false,
      loaded: true,
      error: null,
      refresh: () => Promise.resolve(),
    }),
  ),
}));

vi.mock("@/stores/ui-store", () => ({
  useUIStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({
      setShowNewWorkspaceDialog: vi.fn(),
      setShowWorkspacesOverview: vi.fn(),
    }),
  ),
}));

// Lean row stub — the row's own tests live separately. We only need
// to assert the section reaches the row with the right workspace, so
// render the title in a queryable form.
vi.mock("./workspace-overview-row", () => ({
  WorkspaceOverviewRow: ({
    item,
    isAttached,
  }: {
    item: {
      kind: "local" | "remote";
      key: string;
      workspace?: WorkspaceSnapshot;
      sync?: { title: string; host_server_id: string | null };
    };
    isAttached: boolean;
  }) => {
    const title =
      item.kind === "local"
        ? item.workspace!.title
        : item.sync!.title;
    const wid =
      item.kind === "local" ? item.workspace!.workspace_id : item.key;
    return (
      <div
        data-testid="row"
        data-kind={item.kind}
        data-workspace-id={wid}
        data-attached={isAttached ? "1" : "0"}
      >
        {title}
      </div>
    );
  },
}));

import { WorkspacesOverviewSection } from "./workspaces-overview-section";

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

function makeHost(
  id: number,
  name: string,
  serverId: string | null = `srv-${id}`,
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

describe("WorkspacesOverviewSection", () => {
  beforeEach(() => {
    mockWorkspaces = [];
    mockActiveWorkspaceId = null;
    mockHosts = [];
    mockHostsLoaded = true;
    mockInitHosts.mockClear();
  });

  it("renders the first-run empty state when there are no workspaces", () => {
    mockWorkspaces = [];
    const { getByText } = render(<WorkspacesOverviewSection />);
    expect(getByText("No workspaces yet")).toBeInTheDocument();
  });

  it("renders a loading state while appState is still null", () => {
    mockWorkspaces = null;
    const { getByText } = render(<WorkspacesOverviewSection />);
    expect(getByText(/loading workspaces/i)).toBeInTheDocument();
  });

  it("groups local workspaces under 'This device' and remote ones under the host", () => {
    mockHosts = [makeHost(7, "devbox")];
    mockWorkspaces = [
      makeWorkspace({ workspace_id: "local-1", title: "alpha" }),
      makeWorkspace({
        workspace_id: "remote-1",
        title: "beta",
        host_id: 7,
      }),
    ];
    const { getByText, getAllByTestId } = render(
      <WorkspacesOverviewSection />,
    );

    // Both bucket headers
    expect(getByText("This device")).toBeInTheDocument();
    expect(getByText("devbox")).toBeInTheDocument();

    // Both workspaces rendered via the row stub
    const rows = getAllByTestId("row");
    const ids = rows.map((r) => r.getAttribute("data-workspace-id"));
    expect(ids).toContain("local-1");
    expect(ids).toContain("remote-1");
  });

  it("clusters two+ workspaces of the same project under a project header", () => {
    // Two workspaces sharing a project (same project_root basename
    // 'passpage') should render a 'passpage' group header so the user
    // sees they belong together — the core UX gap this fixes. Titles
    // differ from the project name, so 'passpage' in the DOM can only
    // come from the group header (the row stub renders titles).
    mockWorkspaces = [
      makeWorkspace({
        workspace_id: "ws-root",
        title: "main checkout",
        project_root: "/home/test/passpage",
      }),
      makeWorkspace({
        workspace_id: "ws-wt",
        title: "ui-polish worktree",
        project_root: "/home/test/passpage",
      }),
    ];
    const { getByText } = render(<WorkspacesOverviewSection />);

    // The cluster header.
    expect(getByText("passpage")).toBeInTheDocument();
    // Both workspaces still render.
    expect(getByText("main checkout")).toBeInTheDocument();
    expect(getByText("ui-polish worktree")).toBeInTheDocument();
  });

  it("does not render project headers for one-off projects", () => {
    // Distinct projects (one workspace each) stay in a flat grid with
    // no per-project header — avoids cluttering a device full of
    // unrelated projects. The project names never appear (rows render
    // titles only), so their absence proves no header was emitted.
    mockWorkspaces = [
      makeWorkspace({
        workspace_id: "ws-a",
        title: "alpha-title",
        project_root: "/home/test/proj-a",
      }),
      makeWorkspace({
        workspace_id: "ws-b",
        title: "beta-title",
        project_root: "/home/test/proj-b",
      }),
    ];
    const { getByText, queryByText } = render(<WorkspacesOverviewSection />);

    expect(getByText("alpha-title")).toBeInTheDocument();
    expect(getByText("beta-title")).toBeInTheDocument();
    expect(queryByText("proj-a")).toBeNull();
    expect(queryByText("proj-b")).toBeNull();
  });

  it("marks the active workspace as attached", () => {
    mockActiveWorkspaceId = "local-1";
    mockWorkspaces = [
      makeWorkspace({ workspace_id: "local-1", title: "alpha" }),
      makeWorkspace({ workspace_id: "local-2", title: "gamma" }),
    ];
    const { getAllByTestId } = render(<WorkspacesOverviewSection />);
    const rows = getAllByTestId("row");
    const attachedIds = rows
      .filter((r) => r.getAttribute("data-attached") === "1")
      .map((r) => r.getAttribute("data-workspace-id"));
    expect(attachedIds).toEqual(["local-1"]);
  });

  it("filters by search across title, branch, and project name", () => {
    mockWorkspaces = [
      makeWorkspace({
        workspace_id: "ws-1",
        title: "alpha",
        git_branch: "feature/login",
      }),
      makeWorkspace({
        workspace_id: "ws-2",
        title: "beta",
        git_branch: "main",
      }),
    ];
    const { getByPlaceholderText, queryByText } = render(
      <WorkspacesOverviewSection />,
    );

    const search = getByPlaceholderText(/search by name, branch/i);
    fireEvent.change(search, { target: { value: "login" } });

    // Row stub renders title; ws-2 should be filtered out.
    expect(queryByText("alpha")).toBeInTheDocument();
    expect(queryByText("beta")).toBeNull();
  });

  it("orphan local workspaces (host_id with no matching local host) fall back to the 'This device' bucket", () => {
    // host_id=99 doesn't map to any local hosts row → the host
    // resolver returns null → the workspace lands under "This
    // device" rather than vanishing. This is the local-only
    // fallback; the sibling-device "Host not on this device"
    // bucket is exercised in the workspaces-sync E2E layer.
    mockHosts = [];
    mockWorkspaces = [
      makeWorkspace({
        workspace_id: "orphan-1",
        title: "lost-and-found",
        host_id: 99,
      }),
    ];
    const { getByText } = render(<WorkspacesOverviewSection />);
    expect(getByText("This device")).toBeInTheDocument();
    expect(getByText("lost-and-found")).toBeInTheDocument();
  });

  it("shows a configured host's bucket even when no workspaces live on it yet", () => {
    // Regression for the "I added a device, it doesn't show up in the
    // overview until I push a workspace" bug. The host has a server_id
    // (i.e. it's synced cross-device), there are no workspaces on it,
    // and there's a local workspace so the local bucket is non-empty.
    // The configured-host bucket must still render — otherwise the
    // user has no way to discover the device from the overview, nor
    // any obvious target to push to.
    mockHosts = [makeHost(7, "pandora")];
    mockWorkspaces = [
      makeWorkspace({ workspace_id: "local-1", title: "alpha" }),
    ];
    const { getByText } = render(<WorkspacesOverviewSection />);
    expect(getByText("This device")).toBeInTheDocument();
    expect(getByText("pandora")).toBeInTheDocument();
  });

  it("renders the bucket-level 'all hidden by filter' message when filters hide every workspace in a known device bucket", () => {
    // Intentional: when the user has a bucket they recognise but
    // every row in it is filtered out, the bucket stays visible
    // with an inline hint — so they see *which* device the missing
    // rows belong to, not just "no results."
    mockWorkspaces = [
      makeWorkspace({ workspace_id: "ws-1", title: "alpha" }),
    ];
    const { getByPlaceholderText, getByText } = render(
      <WorkspacesOverviewSection />,
    );

    const search = getByPlaceholderText(/search by name, branch/i);
    fireEvent.change(search, { target: { value: "zzzz-not-real" } });

    expect(
      getByText(/every workspace in This device is hidden/i),
    ).toBeInTheDocument();
    // The bucket header is still there so the user can see which
    // device the filtered rows belong to.
    expect(getByText("This device")).toBeInTheDocument();
  });
});
