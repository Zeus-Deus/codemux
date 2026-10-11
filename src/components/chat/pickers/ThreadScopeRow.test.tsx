/// <reference types="@testing-library/jest-dom/vitest" />
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { BranchDetail, WorkspaceSnapshot } from "@/tauri/types";
import type { HostView } from "@/tauri/commands";
import { host } from "@/components/devices/host-fixtures.test-utils";

// ── App-store mock — keep
// the real grouping helper, stub the store hooks against a
// test-controlled workspace list. ──
let currentWorkspaces: WorkspaceSnapshot[] = [];

function makeWs(overrides: Partial<WorkspaceSnapshot>): WorkspaceSnapshot {
  return {
    workspace_id: "ws-default",
    title: "ws",
    workspace_type: "standard",
    cwd: "/projects/foo",
    git_branch: null,
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    notification_count: 0,
    notifications_muted: false,
    latest_agent_state: null,
    worktree_path: null,
    project_root: "/projects/foo",
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    tabs: [],
    active_tab_id: "",
    active_surface_id: "",
    surfaces: [],
    ...overrides,
  } as WorkspaceSnapshot;
}

vi.mock("@/stores/app-store", async () => {
  const actual =
    await vi.importActual<typeof import("@/stores/app-store")>(
      "@/stores/app-store",
    );
  return {
    ...actual,
    useAppStore: vi.fn((selector: (s: unknown) => unknown) =>
      selector({ appState: { workspaces: currentWorkspaces } }),
    ),
    useHomeDir: () => "/home/user",
    useProjectGroupedWorkspaces: actual.useProjectGroupedWorkspaces,
    resolveProjectRoot: actual.resolveProjectRoot,
  };
});

let currentHosts: HostView[] = [];
vi.mock("@/stores/hosts-store", () => ({
  useHosts: () => currentHosts,
}));
vi.mock("@/stores/host-status-store", () => ({
  useHostStatuses: () => ({}),
}));
vi.mock("@/stores/local-device-store", () => ({
  useLocalDeviceName: () => "ai-node",
}));

vi.mock("@/tauri/commands", () => ({
  listBranchesDetailed: vi.fn(),
  // Probe fallback used by ThreadScopeRow when no workspace row carries
  // the project's `is_git` flag. Defaults to true (git repo) so the
  // checkout/branch controls render as they did pre-probe; individual
  // tests override it to exercise the non-git path.
  checkIsGitRepo: vi.fn().mockResolvedValue(true),
}));

import { ThreadScopeRow, type ThreadScopeRowProps } from "./ThreadScopeRow";
import { listBranchesDetailed } from "@/tauri/commands";

afterEach(() => cleanup());

const NOW = Math.floor(Date.now() / 1000);

function branch(name: string, overrides: Partial<BranchDetail> = {}): BranchDetail {
  return {
    name,
    last_commit_unix: NOW - 3600,
    is_local: true,
    is_remote: false,
    is_head: false,
    ...overrides,
  };
}

function renderRow(overrides: Partial<ThreadScopeRowProps> = {}) {
  const onChangeCheckoutMode = vi.fn();
  const onChangeWorktreeName = vi.fn();
  const onChangeBaseBranch = vi.fn();
  const onChangeHostId = vi.fn();
  const props: ThreadScopeRowProps = {
    projectPath: "/projects/foo",
    hostId: null,
    onChangeHostId,
    checkoutMode: "current",
    worktreeName: "",
    baseBranch: "main",
    onChangeCheckoutMode,
    onChangeWorktreeName,
    onChangeBaseBranch,
    ...overrides,
  };
  const utils = render(<ThreadScopeRow {...props} />);
  return {
    ...utils,
    onChangeCheckoutMode,
    onChangeWorktreeName,
    onChangeBaseBranch,
    onChangeHostId,
  };
}

/** Controlled harness: the real composer stores `checkoutMode` /
 *  `baseBranch` in the draft and feeds them back down, so the pill shows
 *  whatever the row seeds. `renderRow` freezes those props, which is fine
 *  for click assertions but hides the seeding effect's result. */
function renderControlled(
  initial: {
    checkoutMode?: "current" | "worktree";
    baseBranch?: string;
  } = {},
) {
  const onChangeCheckoutMode = vi.fn();
  const onChangeBaseBranch = vi.fn();
  function Harness() {
    const [checkoutMode, setCheckoutMode] = useState<"current" | "worktree">(
      initial.checkoutMode ?? "current",
    );
    const [baseBranch, setBaseBranch] = useState(initial.baseBranch ?? "");
    return (
      <ThreadScopeRow
        projectPath="/projects/foo"
        hostId={null}
        onChangeHostId={vi.fn()}
        checkoutMode={checkoutMode}
        worktreeName=""
        baseBranch={baseBranch}
        onChangeCheckoutMode={(mode) => {
          onChangeCheckoutMode(mode);
          setCheckoutMode(mode);
        }}
        onChangeWorktreeName={vi.fn()}
        onChangeBaseBranch={(name) => {
          onChangeBaseBranch(name);
          setBaseBranch(name);
        }}
      />
    );
  }
  const utils = render(<Harness />);
  return { ...utils, onChangeCheckoutMode, onChangeBaseBranch };
}

describe("ThreadScopeRow", () => {
  beforeEach(() => {
    currentWorkspaces = [];
    currentHosts = [];
    vi.mocked(listBranchesDetailed).mockReset().mockResolvedValue([
      branch("main", { last_commit_unix: NOW - 3600 }),
      branch("develop", { last_commit_unix: NOW - 86400 }),
    ]);
  });

  // The project is picked in the new-thread headline, so the strip never
  // carries its own project picker.
  describe("home target", () => {
    it("renders only the device control — no project, checkout, or branch controls", () => {
      const { container } = renderRow({ projectPath: null });
      expect(
        screen.getByRole("button", { name: "Device: ai-node" }),
      ).toBeInTheDocument();
      expect(screen.getAllByRole("button")).toHaveLength(1);
      expect(screen.queryByText("Home")).toBeNull();
      expect(screen.queryByText("Current checkout")).toBeNull();
      expect(screen.queryByText("New worktree")).toBeNull();
      expect(screen.queryByText(/^from$/)).toBeNull();
      // No dangling separator after the lone device control.
      expect(container.textContent).not.toContain("·");
    });
  });

  describe("device control", () => {
    it("leads the strip with this machine's name, ahead of the checkout control", () => {
      renderRow();
      const device = screen.getByRole("button", { name: "Device: ai-node" });
      const checkout = screen.getByText("Current checkout").closest("button")!;
      expect(
        device.compareDocumentPosition(checkout) &
          Node.DOCUMENT_POSITION_FOLLOWING,
      ).toBeTruthy();
    });

    it("shows the picked device's name", () => {
      currentHosts = [host(2, "zeus")];
      renderRow({ hostId: 2 });
      expect(
        screen.getByRole("button", { name: "Device: zeus" }),
      ).toBeInTheDocument();
    });

    it("picking a device reports its id and keeps the checkout controls", async () => {
      const user = userEvent.setup();
      currentHosts = [host(2, "zeus")];
      const { onChangeHostId } = renderRow();
      await user.click(screen.getByRole("button", { name: "Device: ai-node" }));
      const row = await waitFor(() => {
        const el = document.querySelector<HTMLElement>('[data-host-id="2"]');
        expect(el).not.toBeNull();
        return el!;
      });
      await user.click(row);
      expect(onChangeHostId).toHaveBeenCalledWith(2);
      expect(screen.getByText("Current checkout")).toBeInTheDocument();
    });

    it("names the device's checkout instead of this machine's branch in its current checkout", async () => {
      vi.mocked(listBranchesDetailed).mockResolvedValue([
        branch("main"),
        branch("feature-x", { is_head: true }),
      ]);
      currentHosts = [host(2, "zeus")];
      const { onChangeBaseBranch } = renderRow({
        hostId: 2,
        checkoutMode: "current",
        baseBranch: "feature-x",
      });

      expect(screen.getByText("zeus's checkout")).toBeInTheDocument();
      expect(screen.queryByText("feature-x")).toBeNull();
      expect(screen.queryByText(/^from$/)).toBeNull();
      // Read-only: the device's branch can't be picked from here.
      expect(screen.getByText("zeus's checkout").closest("button")).toBeNull();
      // This machine's branches are never read for it.
      await Promise.resolve();
      expect(listBranchesDetailed).not.toHaveBeenCalled();
      expect(onChangeBaseBranch).not.toHaveBeenCalled();
    });

    it("keeps the base-branch pill for a new worktree on a device", () => {
      currentHosts = [host(2, "zeus")];
      renderRow({ hostId: 2, checkoutMode: "worktree", baseBranch: "develop" });
      expect(screen.getByText("develop")).toBeInTheDocument();
      expect(screen.queryByText("zeus's checkout")).toBeNull();
    });
  });

  describe("project target — current checkout", () => {
    it("renders checkout and branch controls, without a project picker", () => {
      renderRow();
      expect(screen.getByText("Current checkout")).toBeInTheDocument();
      expect(screen.getByText("main")).toBeInTheDocument();
      expect(screen.queryByText("foo")).toBeNull();
    });

    it("renders the worktree checkout control when checkoutMode is 'worktree'", () => {
      renderRow({ checkoutMode: "worktree", baseBranch: "develop" });
      expect(screen.getByText("New worktree")).toBeInTheDocument();
    });

    it("hides checkout/branch controls when projectPath hasn't resolved yet", () => {
      renderRow({ projectPath: null });
      expect(screen.queryByText("Current checkout")).toBeNull();
      expect(screen.queryByText(/^from$/)).toBeNull();
    });
  });

  describe("checkout control", () => {
    it("selecting 'New worktree' calls onChangeCheckoutMode('worktree')", async () => {
      const user = userEvent.setup();
      const { onChangeCheckoutMode } = renderRow();
      await user.click(screen.getByText("Current checkout"));
      await screen.findByText("Where should the agent work?");
      await user.click(screen.getByText("New worktree"));
      expect(onChangeCheckoutMode).toHaveBeenCalledWith("worktree");
    });

    it("shows the name input + hint only when checkoutMode is 'worktree', and typing calls onChangeWorktreeName", async () => {
      const user = userEvent.setup();
      const { onChangeWorktreeName } = renderRow({ checkoutMode: "worktree" });
      await user.click(screen.getByText("New worktree"));
      const input = await screen.findByPlaceholderText(
        "name — leave empty to auto-name",
      );
      await user.type(input, "x");
      expect(onChangeWorktreeName).toHaveBeenCalledWith("x");
      expect(
        screen.getByText(/CodeMux names it from your first message/i),
      ).toBeInTheDocument();
    });
  });

  describe("branch control", () => {
    it("picking a DIFFERENT branch while on 'current' checkout flips to 'worktree' with that branch as base", async () => {
      const user = userEvent.setup();
      const { onChangeCheckoutMode, onChangeBaseBranch } = renderRow({
        checkoutMode: "current",
        baseBranch: "main",
      });
      await user.click(screen.getByText("main"));
      const developRow = await screen.findByText("develop");
      await user.click(developRow);
      expect(onChangeCheckoutMode).toHaveBeenCalledWith("worktree");
      expect(onChangeBaseBranch).toHaveBeenCalledWith("develop");
    });

    it("picking the SAME branch while on 'current' checkout does not flip checkoutMode", async () => {
      const user = userEvent.setup();
      const { onChangeCheckoutMode, onChangeBaseBranch } = renderRow({
        checkoutMode: "current",
        baseBranch: "main",
      });
      await user.click(screen.getByText("main"));
      const rows = await screen.findAllByText("main");
      // Click the row inside the popover list (not the trigger).
      await user.click(rows[rows.length - 1]);
      expect(onChangeCheckoutMode).not.toHaveBeenCalled();
      expect(onChangeBaseBranch).toHaveBeenCalledWith("main");
    });

    it("picking a branch while already on 'worktree' checkout just updates the base branch", async () => {
      const user = userEvent.setup();
      const { onChangeCheckoutMode, onChangeBaseBranch } = renderRow({
        checkoutMode: "worktree",
        baseBranch: "main",
      });
      await user.click(screen.getByText("main"));
      const developRow = await screen.findByText("develop");
      await user.click(developRow);
      expect(onChangeCheckoutMode).not.toHaveBeenCalled();
      expect(onChangeBaseBranch).toHaveBeenCalledWith("develop");
    });

    it("seeds the pill from the checked-out branch, not main", async () => {
      vi.mocked(listBranchesDetailed).mockResolvedValue([
        branch("main"),
        branch("feature-x", { is_head: true }),
        branch("develop"),
      ]);
      const { onChangeBaseBranch } = renderControlled();
      expect(await screen.findByText("feature-x")).toBeInTheDocument();
      expect(onChangeBaseBranch).toHaveBeenCalledWith("feature-x");
      expect(onChangeBaseBranch).not.toHaveBeenCalledWith("main");
    });

    it("falls back to main when no branch is flagged as checked out", async () => {
      vi.mocked(listBranchesDetailed).mockResolvedValue([
        branch("feature-x"),
        branch("main"),
        branch("develop"),
      ]);
      const { onChangeBaseBranch } = renderControlled();
      await waitFor(() => {
        expect(onChangeBaseBranch).toHaveBeenCalledWith("main");
      });
      expect(screen.getByText("main")).toBeInTheDocument();
    });

    it("picking main while the checkout is on feature-x flips to a worktree based on main", async () => {
      vi.mocked(listBranchesDetailed).mockResolvedValue([
        branch("feature-x", { is_head: true }),
        branch("main"),
      ]);
      const user = userEvent.setup();
      const { onChangeCheckoutMode, onChangeBaseBranch } = renderControlled();
      await user.click(await screen.findByText("feature-x"));
      await user.click(await screen.findByText("main"));
      expect(onChangeCheckoutMode).toHaveBeenCalledWith("worktree");
      expect(onChangeBaseBranch).toHaveBeenLastCalledWith("main");
      expect(await screen.findByText("New worktree")).toBeInTheDocument();
    });

    it("switching back to the current checkout snaps the pill back to the real HEAD", async () => {
      vi.mocked(listBranchesDetailed).mockResolvedValue([
        branch("feature-x", { is_head: true }),
        branch("main"),
      ]);
      const user = userEvent.setup();
      const { onChangeBaseBranch } = renderControlled();
      await user.click(await screen.findByText("feature-x"));
      await user.click(await screen.findByText("main"));
      await screen.findByText("New worktree");

      await user.click(screen.getByText("New worktree"));
      await screen.findByText("Where should the agent work?");
      await user.click(screen.getByText("Current checkout"));

      await waitFor(() => {
        expect(onChangeBaseBranch).toHaveBeenLastCalledWith("feature-x");
      });
      expect(screen.getByText("feature-x")).toBeInTheDocument();
    });

    it("refetches on a project switch, so the pill can't keep the old project's HEAD", async () => {
      vi.mocked(listBranchesDetailed).mockImplementation(async (path: string) =>
        path === "/projects/bar"
          ? [branch("bar-head", { is_head: true })]
          : [branch("foo-head", { is_head: true })],
      );
      const onChangeBaseBranch = vi.fn();
      const shared = {
        hostId: null,
        onChangeHostId: vi.fn(),
        checkoutMode: "current",
        worktreeName: "",
        baseBranch: "",
        onChangeCheckoutMode: vi.fn(),
        onChangeWorktreeName: vi.fn(),
        onChangeBaseBranch,
      } satisfies Partial<ThreadScopeRowProps>;

      const { rerender } = render(
        <ThreadScopeRow {...shared} projectPath="/projects/foo" />,
      );
      await waitFor(() => {
        expect(onChangeBaseBranch).toHaveBeenLastCalledWith("foo-head");
      });

      rerender(
        <ThreadScopeRow {...shared} projectPath="/projects/bar" />,
      );
      await waitFor(() => {
        expect(onChangeBaseBranch).toHaveBeenLastCalledWith("bar-head");
      });
    });

    it("shows a WORKTREE badge on branches that have a worktree on this device", async () => {
      currentWorkspaces = [
        makeWs({
          workspace_id: "ws-foo-main",
          cwd: "/projects/foo",
          project_root: "/projects/foo",
          git_branch: "main",
        }),
      ];
      const user = userEvent.setup();
      renderRow();
      await user.click(screen.getByText("main"));
      await waitFor(() => {
        expect(screen.getByText("WORKTREE")).toBeInTheDocument();
      });
    });
  });
});
