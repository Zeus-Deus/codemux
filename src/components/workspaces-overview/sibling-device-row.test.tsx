/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render } from "@testing-library/react";

import type { WorkspaceSyncView } from "@/tauri/commands";
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

  it("renders a 'main' kind badge for the root checkout", () => {
    const { getByText } = render(
      <WorkspaceOverviewRow
        item={remoteItem({ workspace_kind: "main" })}
        isAttached={false}
        onAfterOpen={() => {}}
      />,
    );
    expect(getByText("main")).toBeInTheDocument();
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
