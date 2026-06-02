/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render } from "@testing-library/react";

import type { WorkspaceSyncView } from "@/tauri/commands";
import type { WorkspaceSnapshot } from "@/tauri/types";
import type { OverviewItem } from "./use-overview-items";

// The row pulls hosts + push/pull commands. Stub everything the
// remote branch doesn't actually need, so the test stays narrow.
vi.mock("@/stores/hosts-store", () => ({
  useHostsStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({ hosts: [], loaded: true, init: () => Promise.resolve() }),
  ),
  useHosts: () => [],
}));
vi.mock("@/stores/app-store", () => ({
  useAppStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({
      appState: null,
      workspacePushPullInFlight: null,
      setWorkspacePushPullInFlight: vi.fn(),
    }),
  ),
}));
vi.mock("@/stores/ui-store", () => ({
  useUIStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({ setShowWorkspacesOverview: vi.fn() }),
  ),
}));
vi.mock("@/lib/pane-status", () => ({
  getWorkspaceStatus: () => null,
}));

import { WorkspaceOverviewRow } from "./workspace-overview-row";

function makeSyncRow(overrides?: Partial<WorkspaceSyncView>): WorkspaceSyncView {
  return {
    id: 1,
    server_id: "42",
    workspace_id: null,
    title: "remote-only-workspace",
    host_server_id: "7",
    project_path: "/home/zeus/projects/codemux",
    project_remote: "git@github.com:Zeus-Deus/codemux.git",
    git_branch: "feature/cross-device",
    git_head_sha: null,
    project_uid: null,
    workspace_kind: null,
    created_at: "2026-05-20T10:00:00Z",
    updated_at: "2026-05-20T10:00:00Z",
    dirty: false,
    ...overrides,
  };
}

function remoteItem(overrides?: Partial<WorkspaceSyncView>): OverviewItem {
  const sync = makeSyncRow(overrides);
  return {
    kind: "remote",
    key: `remote:${sync.server_id ?? sync.id}`,
    sync,
    hostServerId: sync.host_server_id,
    projectName: sync.project_path
      ? sync.project_path.split("/").filter(Boolean).slice(-1)[0] ?? null
      : null,
    projectPath: sync.project_path,
    projectKey: sync.project_uid ?? sync.project_path,
  };
}

afterEach(() => cleanup());

describe("WorkspaceOverviewRow (sibling-device branch)", () => {
  it("renders the title, project, and the 'other device' affordance", () => {
    const { getByText } = render(
      <WorkspaceOverviewRow
        item={remoteItem()}
        isAttached={false}
        onAfterOpen={() => {}}
      />,
    );
    expect(getByText("remote-only-workspace")).toBeInTheDocument();
    // The pill that signals cross-device.
    expect(getByText(/other device/i)).toBeInTheDocument();
    // The "not on this device" sub-label.
    expect(getByText(/not on this device/i)).toBeInTheDocument();
    // Project name derived from project_path basename.
    expect(getByText("codemux")).toBeInTheDocument();
  });

  it("renders a 'worktree' kind badge when the row is typed", () => {
    const { getByText } = render(
      <WorkspaceOverviewRow
        item={remoteItem({ workspace_kind: "worktree" })}
        isAttached={false}
        onAfterOpen={() => {}}
      />,
    );
    expect(getByText("worktree")).toBeInTheDocument();
  });

  it("renders a 'repo root' kind badge for the root checkout", () => {
    const { getByText } = render(
      <WorkspaceOverviewRow
        item={remoteItem({ workspace_kind: "main" })}
        isAttached={false}
        onAfterOpen={() => {}}
      />,
    );
    // The "main" kind renders with the friendlier "repo root" label.
    expect(getByText("repo root")).toBeInTheDocument();
  });

  it("renders the branch on the bottom row", () => {
    const { getByText } = render(
      <WorkspaceOverviewRow
        item={remoteItem({ git_branch: "feature/x" })}
        isAttached={false}
        onAfterOpen={() => {}}
      />,
    );
    expect(getByText("feature/x")).toBeInTheDocument();
  });

  it("does not render a click-to-open handler — sibling rows are read-only in v1", () => {
    const onAfterOpen = vi.fn();
    const { container } = render(
      <WorkspaceOverviewRow
        item={remoteItem()}
        isAttached={false}
        onAfterOpen={onAfterOpen}
      />,
    );
    // Top-level wrapper is a `role="group"`, not `role="button"`.
    const wrapper = container.firstChild as HTMLElement;
    expect(wrapper.getAttribute("role")).toBe("group");
    // No click should fire onAfterOpen.
    wrapper.click();
    expect(onAfterOpen).not.toHaveBeenCalled();
  });

  it("title omits a branch slot when git_branch is null", () => {
    const { queryByText } = render(
      <WorkspaceOverviewRow
        item={remoteItem({ git_branch: null })}
        isAttached={false}
        onAfterOpen={() => {}}
      />,
    );
    // Sanity — title still renders.
    expect(queryByText("remote-only-workspace")).toBeInTheDocument();
    // The branch row only renders when git_branch is non-null.
    expect(queryByText("feature/cross-device")).toBeNull();
  });
});

function localItem(
  ws: Partial<WorkspaceSnapshot> & { workspace_id: string },
): OverviewItem {
  const workspace = {
    workspace_id: ws.workspace_id,
    title: ws.title ?? ws.workspace_id,
    workspace_type: "standard",
    cwd: "/home/test/proj",
    git_branch: ws.git_branch ?? "main",
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    notification_count: 0,
    latest_agent_state: null,
    worktree_path: ws.worktree_path ?? null,
    project_root: "/home/test/proj",
    workspace_kind: ws.workspace_kind,
    protected: ws.protected,
    divergent_copy: ws.divergent_copy,
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    notifications_muted: false,
    tabs: [],
    active_tab_id: "",
    active_surface_id: "",
    surfaces: [],
    host_id: null,
  } as unknown as WorkspaceSnapshot;
  return {
    kind: "local",
    key: `local:${workspace.workspace_id}`,
    workspace,
    sync: null,
    hostServerId: null,
    projectName: "proj",
    projectPath: "/home/test/proj",
    projectKey: "/home/test/proj",
  };
}

describe("WorkspaceOverviewRow (local repo-root protection)", () => {
  it("renders a 'repo root' badge for a protected root checkout", () => {
    const { getByText } = render(
      <WorkspaceOverviewRow
        item={localItem({
          workspace_id: "ws-root",
          title: "passpage",
          protected: true,
          workspace_kind: "main",
        })}
        isAttached={false}
        onAfterOpen={() => {}}
      />,
    );
    expect(getByText("repo root")).toBeInTheDocument();
  });

  it("does NOT render a 'repo root' badge for a worktree", () => {
    const { queryByText } = render(
      <WorkspaceOverviewRow
        item={localItem({
          workspace_id: "ws-wt",
          title: "feature-x",
          protected: false,
          worktree_path: "/home/test/.codemux/worktrees/proj/feature-x",
          workspace_kind: "worktree",
        })}
        isAttached={false}
        onAfterOpen={() => {}}
      />,
    );
    expect(queryByText("repo root")).toBeNull();
  });

  it("warns with a 'standalone copy' chip for a divergent copy", () => {
    const { getByText, queryByText } = render(
      <WorkspaceOverviewRow
        item={localItem({
          workspace_id: "ws-copy",
          title: "passpage",
          // A divergent copy is NOT protected (so it can be reconciled)
          // and is flagged by the backend's divergent_copy stamp.
          protected: false,
          divergent_copy: true,
          worktree_path: "/home/test/.codemux/worktrees/passpage/main",
        })}
        isAttached={false}
        onAfterOpen={() => {}}
      />,
    );
    expect(getByText("standalone copy")).toBeInTheDocument();
    // It is not advertised as a protected repo root.
    expect(queryByText("repo root")).toBeNull();
  });
});
