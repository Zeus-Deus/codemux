/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, act, cleanup, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

// Polyfill ResizeObserver
globalThis.ResizeObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

// ── Mock Tauri commands ──

const mockListIncomingPrs = vi.fn().mockResolvedValue([]);
const mockActivateWorkspace = vi.fn().mockResolvedValue(undefined);
const mockCreateWorktreeWorkspace = vi.fn().mockResolvedValue("ws-new");

const mockOpenUrl = vi.fn();
vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: (...args: unknown[]) => mockOpenUrl(...args),
}));

const mockToastError = vi.fn();
vi.mock("@/lib/toast", () => ({
  toast: { error: (...args: unknown[]) => mockToastError(...args) },
}));

vi.mock("@/tauri/commands", () => ({
  listIncomingPrs: (...args: unknown[]) => mockListIncomingPrs(...args),
  activateWorkspace: (...args: unknown[]) => mockActivateWorkspace(...args),
  createWorktreeWorkspace: (...args: unknown[]) => mockCreateWorktreeWorkspace(...args),
}));

// ── Mock app store ──

const mockWorkspaces: { workspace_id: string; git_branch: string | null; project_root: string | null }[] = [];

// Only the hook's projection is faked; the real selectors and `getState` stay,
// because the activation helper reads both.
vi.mock("@/stores/app-store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/stores/app-store")>();
  return {
    ...actual,
    useAppStore: Object.assign(
      (selector: (s: Record<string, unknown>) => unknown) =>
        selector({ appState: { workspaces: mockWorkspaces } }),
      { getState: actual.useAppStore.getState },
    ),
  };
});

import { IncomingPrsView, _resetIncomingPrsCache } from "./incoming-prs-view";
import type { IncomingPrItem } from "@/tauri/types";

function flushPromises() {
  return act(() => new Promise((r) => setTimeout(r, 0)));
}

const samplePrs: IncomingPrItem[] = [
  {
    number: 27,
    title: "Add user settings page",
    author: "alice",
    head_branch: "feat/settings",
    is_draft: false,
    additions: 110,
    deletions: 26,
    review_decision: "APPROVED",
    checks_status: "success",
    updated_at: new Date(Date.now() - 2 * 24 * 3600 * 1000).toISOString(),
    url: "https://github.com/test/repo/pull/27",
  },
  {
    number: 28,
    title: "Fix login bug",
    author: "bob",
    head_branch: "fix/login",
    is_draft: true,
    additions: 5,
    deletions: 3,
    review_decision: null,
    checks_status: "failure",
    updated_at: new Date(Date.now() - 3600 * 1000).toISOString(),
    url: "https://github.com/test/repo/pull/28",
  },
  {
    number: 29,
    title: "Update docs",
    author: "carol",
    head_branch: "docs/update",
    is_draft: false,
    additions: 0,
    deletions: 0,
    review_decision: "CHANGES_REQUESTED",
    checks_status: "pending",
    updated_at: new Date(Date.now() - 10 * 60 * 1000).toISOString(),
    url: "https://github.com/test/repo/pull/29",
  },
];

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  mockWorkspaces.length = 0;
  mockListIncomingPrs.mockResolvedValue([]);
  // The view caches results module-level (so a re-mounted Review tab
  // paints instantly instead of re-shelling-out to `gh`). Reset
  // between tests so a previous test's PR list doesn't bleed into
  // the next render.
  _resetIncomingPrsCache();
});

afterEach(() => {
  vi.useRealTimers();
});

const defaultProps = {
  cwd: "/home/user/project",
  baseBranch: "main",
  projectRoot: "/home/user/project",
  refreshKey: 0,
};

describe("IncomingPrsView", () => {
  it("renders PR list with correct number, title, author", async () => {
    mockListIncomingPrs.mockResolvedValue(samplePrs);
    render(<IncomingPrsView {...defaultProps} />);
    await flushPromises();

    expect(screen.getByText("#27")).toBeInTheDocument();
    expect(screen.getByText("Add user settings page")).toBeInTheDocument();
    expect(screen.getByText("#28")).toBeInTheDocument();
    expect(screen.getByText("Fix login bug")).toBeInTheDocument();
    expect(screen.getByText("#29")).toBeInTheDocument();
    expect(screen.getByText("Update docs")).toBeInTheDocument();
  });

  it("shows empty state when no PRs", async () => {
    mockListIncomingPrs.mockResolvedValue([]);
    render(<IncomingPrsView {...defaultProps} />);
    await flushPromises();

    expect(screen.getByText("No open pull requests")).toBeInTheDocument();
  });

  it("shows error banner on fetch failure", async () => {
    mockListIncomingPrs.mockRejectedValue(new Error("gh failed"));
    render(<IncomingPrsView {...defaultProps} />);
    await flushPromises();

    await waitFor(() => {
      expect(screen.getByText(/gh failed/)).toBeInTheDocument();
    });
  });

  it("shows loading skeleton initially", () => {
    mockListIncomingPrs.mockReturnValue(new Promise(() => {})); // never resolves
    render(<IncomingPrsView {...defaultProps} />);

    // Should show skeleton pulse elements
    const skeletons = document.querySelectorAll(".animate-pulse");
    expect(skeletons.length).toBeGreaterThan(0);
  });

  it("shows count badge with PR count", async () => {
    mockListIncomingPrs.mockResolvedValue(samplePrs);
    render(<IncomingPrsView {...defaultProps} />);
    await flushPromises();

    expect(screen.getByText("3")).toBeInTheDocument();
  });

  it("shows Draft badge for draft PRs", async () => {
    mockListIncomingPrs.mockResolvedValue(samplePrs);
    render(<IncomingPrsView {...defaultProps} />);
    await flushPromises();

    expect(screen.getByText("Draft")).toBeInTheDocument();
  });

  it("shows review decision badges", async () => {
    mockListIncomingPrs.mockResolvedValue(samplePrs);
    render(<IncomingPrsView {...defaultProps} />);
    await flushPromises();

    expect(screen.getByText("Approved")).toBeInTheDocument();
    expect(screen.getByText("Changes")).toBeInTheDocument();
  });

  it("re-fetches when refreshKey changes", async () => {
    mockListIncomingPrs.mockResolvedValue([]);
    const { rerender } = render(<IncomingPrsView {...defaultProps} />);
    await flushPromises();

    expect(mockListIncomingPrs).toHaveBeenCalledTimes(1);

    rerender(<IncomingPrsView {...defaultProps} refreshKey={1} />);
    await flushPromises();

    expect(mockListIncomingPrs).toHaveBeenCalledTimes(2);
  });

  it("shows Checkout button when no existing worktree", async () => {
    mockListIncomingPrs.mockResolvedValue([samplePrs[0]]);
    render(<IncomingPrsView {...defaultProps} />);
    await flushPromises();

    expect(screen.getByText("Checkout")).toBeInTheDocument();
  });

  it("shows Switch button when worktree exists for that branch", async () => {
    mockWorkspaces.push({
      workspace_id: "ws-existing",
      git_branch: "feat/settings",
      project_root: "/home/user/project",
    });

    mockListIncomingPrs.mockResolvedValue([samplePrs[0]]);
    render(<IncomingPrsView {...defaultProps} />);
    await flushPromises();

    expect(screen.getByText("Switch")).toBeInTheDocument();
    expect(screen.queryByText("Checkout")).not.toBeInTheDocument();
  });

  it("calls activateWorkspace when clicking Switch", async () => {
    const user = userEvent.setup();
    mockWorkspaces.push({
      workspace_id: "ws-existing",
      git_branch: "feat/settings",
      project_root: "/home/user/project",
    });

    mockListIncomingPrs.mockResolvedValue([samplePrs[0]]);
    render(<IncomingPrsView {...defaultProps} />);
    await flushPromises();

    await user.click(screen.getByText("Switch"));
    expect(mockActivateWorkspace).toHaveBeenCalledWith("ws-existing");
  });

  it("calls createWorktreeWorkspace when clicking Checkout", async () => {
    const user = userEvent.setup();
    mockListIncomingPrs.mockResolvedValue([samplePrs[0]]);
    render(<IncomingPrsView {...defaultProps} />);
    await flushPromises();

    await user.click(screen.getByText("Checkout"));
    expect(mockCreateWorktreeWorkspace).toHaveBeenCalledWith(
      "/home/user/project",
      "feat/settings",
      false,
      "single",
      null,
      null,
      null,
      27,
    );
  });

  it("shows toast on checkout error", async () => {
    const user = userEvent.setup();
    mockCreateWorktreeWorkspace.mockRejectedValueOnce("branch already checked out");
    mockListIncomingPrs.mockResolvedValue([samplePrs[0]]);
    render(<IncomingPrsView {...defaultProps} />);
    await flushPromises();

    await user.click(screen.getByText("Checkout"));
    await flushPromises();

    expect(mockToastError).toHaveBeenCalledWith("branch already checked out");
  });

  it("calls listIncomingPrs with correct params", async () => {
    render(<IncomingPrsView {...defaultProps} />);
    await flushPromises();

    expect(mockListIncomingPrs).toHaveBeenCalledWith("/home/user/project", "main");
  });

  it("clicking row switches to existing worktree workspace", async () => {
    const user = userEvent.setup();
    mockWorkspaces.push({
      workspace_id: "ws-existing",
      git_branch: "feat/settings",
      project_root: "/home/user/project",
    });

    mockListIncomingPrs.mockResolvedValue([samplePrs[0]]);
    render(<IncomingPrsView {...defaultProps} />);
    await flushPromises();

    // Click the row itself (title text), not a button
    await user.click(screen.getByText("Add user settings page"));
    expect(mockActivateWorkspace).toHaveBeenCalledWith("ws-existing");
  });

  it("View button opens PR URL via openUrl", async () => {
    const user = userEvent.setup();
    mockListIncomingPrs.mockResolvedValue([samplePrs[0]]);
    render(<IncomingPrsView {...defaultProps} />);
    await flushPromises();

    await user.click(screen.getByText("View"));
    expect(mockOpenUrl).toHaveBeenCalledWith("https://github.com/test/repo/pull/27");
  });

  it("clicking row does nothing when no matching worktree", async () => {
    const user = userEvent.setup();
    mockListIncomingPrs.mockResolvedValue([samplePrs[0]]);
    render(<IncomingPrsView {...defaultProps} />);
    await flushPromises();

    await user.click(screen.getByText("Add user settings page"));
    expect(mockActivateWorkspace).not.toHaveBeenCalled();
    expect(mockCreateWorktreeWorkspace).not.toHaveBeenCalled();
  });
});
