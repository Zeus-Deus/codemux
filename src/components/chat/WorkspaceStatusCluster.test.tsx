/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, cleanup, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type {
  AgentBrowserSession,
  GitHubIssue,
  PullRequestInfo,
  WorkspaceSnapshot,
} from "@/tauri/types";

// ── Mocks — `vi.mock()` factories are hoisted, so mutable state they
// close over lives in `vi.hoisted`. ──
const mocks = vi.hoisted(() => ({
  workspace: null as WorkspaceSnapshot | null,
  hosts: [] as Array<{ id: number; name: string }>,
  openUrl: vi.fn().mockResolvedValue(undefined),
  getGithubPrByPath: vi.fn().mockResolvedValue(null as PullRequestInfo | null),
  getGithubIssue: vi.fn().mockResolvedValue(null as GitHubIssue | null),
  gitPullChanges: vi.fn().mockResolvedValue(undefined),
  agentBrowserSessions: [] as AgentBrowserSession[],
}));

vi.mock("@/stores/app-store", () => ({
  useActiveWorkspace: () => mocks.workspace,
  // `useBackgroundBrowserSession` (shared with the terminal header) reads
  // `agent_browser_sessions` off the app state via `useAppStore`.
  useAppStore: (sel: (s: Record<string, unknown>) => unknown) =>
    sel({
      appState: { agent_browser_sessions: mocks.agentBrowserSessions },
    }),
}));
vi.mock("@/stores/hosts-store", () => ({
  useHosts: () => mocks.hosts,
}));
vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: (...args: unknown[]) => mocks.openUrl(...args),
}));
vi.mock("@/tauri/commands", () => ({
  getGithubPrByPath: (...args: unknown[]) => mocks.getGithubPrByPath(...args),
  getGithubIssue: (...args: unknown[]) => mocks.getGithubIssue(...args),
  gitPullChanges: (...args: unknown[]) => mocks.gitPullChanges(...args),
}));
vi.mock("@/lib/toast", () => ({
  toast: { error: vi.fn(), success: vi.fn(), info: vi.fn(), warning: vi.fn() },
}));

import { WorkspaceStatusCluster } from "./WorkspaceStatusCluster";
import { toast } from "@/lib/toast";
import { useBrowserPeekStore } from "@/stores/browser-peek-store";

function makeWorkspace(
  overrides: Partial<WorkspaceSnapshot> = {},
): WorkspaceSnapshot {
  return {
    workspace_id: "ws-1",
    title: "feature/19-cloud-push",
    workspace_type: "standard",
    cwd: "/home/dev/.codemux/worktrees/repo/feature-19",
    git_branch: "feature/19-cloud-push",
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    notification_count: 0,
    latest_agent_state: null,
    worktree_path: "/home/dev/.codemux/worktrees/repo/feature-19",
    project_root: "/home/dev/projects/repo",
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    notifications_muted: false,
    tabs: [],
    active_tab_id: "",
    active_surface_id: "",
    surfaces: [],
    ...overrides,
  };
}

// A live agent browser session detached from any pane (the "background" state).
function makeBackgroundSession(
  overrides: Partial<AgentBrowserSession> = {},
): AgentBrowserSession {
  return {
    session_id: "abs-1",
    workspace_id: "ws-1",
    cli_session_name: "ws-abc123",
    stream_url: "ws://localhost:9223",
    current_url: "https://example.com",
    is_active: true,
    pane_id: null,
    browser_id: null,
    user_dismissed: false,
    ...overrides,
  };
}

beforeEach(() => {
  mocks.workspace = null;
  mocks.hosts = [];
  mocks.openUrl.mockClear();
  mocks.getGithubPrByPath.mockReset().mockResolvedValue(null);
  mocks.getGithubIssue.mockReset().mockResolvedValue(null);
  mocks.gitPullChanges.mockReset().mockResolvedValue(undefined);
  mocks.agentBrowserSessions = [];
  vi.mocked(toast.error).mockClear();
  useBrowserPeekStore.setState({ openWorkspaceId: null });
});

afterEach(cleanup);

describe("WorkspaceStatusCluster", () => {
  it("renders nothing when there is no active workspace", () => {
    const { container } = render(<WorkspaceStatusCluster />);
    expect(container).toBeEmptyDOMElement();
  });

  it("renders nothing when the workspace has no git branch (and no background browser)", () => {
    mocks.workspace = makeWorkspace({ git_branch: null });
    const { container } = render(<WorkspaceStatusCluster />);
    expect(container).toBeEmptyDOMElement();
  });

  it("shows the behind chip with a warning tone when behind > 0", () => {
    mocks.workspace = makeWorkspace({ git_behind: 5 });
    render(<WorkspaceStatusCluster />);
    const chip = screen.getByText("5");
    expect(chip.closest("span")).toHaveClass("text-warning");
  });

  it("hides the behind chip when behind is 0", () => {
    mocks.workspace = makeWorkspace({ git_behind: 0 });
    render(<WorkspaceStatusCluster />);
    expect(screen.queryByTitle(/commits? behind/)).not.toBeInTheDocument();
  });

  it("shows the PR chip tone-tinted and opens the PR on click", async () => {
    mocks.workspace = makeWorkspace({
      pr_number: 172,
      pr_state: "open",
      pr_url: "https://github.com/org/repo/pull/172",
    });
    render(<WorkspaceStatusCluster />);
    const chip = screen.getByRole("button", { name: /Open PR #172 on GitHub/ });
    expect(chip).toHaveTextContent("#172");
    expect(chip).toHaveClass("text-status-open");
    await userEvent.click(chip);
    expect(mocks.openUrl).toHaveBeenCalledWith(
      "https://github.com/org/repo/pull/172",
    );
  });

  it("always renders the workspace-details button", () => {
    mocks.workspace = makeWorkspace();
    render(<WorkspaceStatusCluster />);
    expect(
      screen.getByRole("button", { name: "Workspace details" }),
    ).toBeInTheDocument();
  });

  it("opens the popover with branch, behind, ahead, uncommitted, PR, and location rows", async () => {
    mocks.workspace = makeWorkspace({
      git_behind: 5,
      git_ahead: 2,
      git_additions: 9,
      git_deletions: 1,
      pr_number: 172,
      pr_state: "open",
      pr_url: "https://github.com/org/repo/pull/172",
    });
    const user = userEvent.setup();
    render(<WorkspaceStatusCluster />);
    await user.click(screen.getByRole("button", { name: "Workspace details" }));
    expect(screen.getByText("Branch")).toBeInTheDocument();
    // The header title and the Branch row's value are both
    // "feature/19-cloud-push" in this fixture (a workspace named after
    // its branch, matching the retired bar's visibility contract).
    expect(screen.getAllByText("feature/19-cloud-push")).toHaveLength(2);
    expect(screen.getByText("Behind base")).toBeInTheDocument();
    expect(screen.getByText("↓5")).toBeInTheDocument();
    expect(screen.getByText("Ahead")).toBeInTheDocument();
    expect(screen.getByText("↑2")).toBeInTheDocument();
    expect(screen.getByText("Uncommitted")).toBeInTheDocument();
    expect(screen.getByText("+9 −1")).toBeInTheDocument();
    expect(screen.getByText("Pull request")).toBeInTheDocument();
    expect(screen.getByText("#172 · Open")).toBeInTheDocument();
    expect(screen.getByText("Location")).toBeInTheDocument();
    expect(screen.getByText("this device")).toBeInTheDocument();
  });

  it("fetches the PR detail on open and shows the Base row once resolved", async () => {
    mocks.workspace = makeWorkspace({ pr_number: 172, pr_state: "open" });
    mocks.getGithubPrByPath.mockResolvedValue({
      base_branch: "main",
    } as PullRequestInfo);
    const user = userEvent.setup();
    render(<WorkspaceStatusCluster />);
    expect(screen.queryByText("Base")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Workspace details" }));
    await waitFor(() => expect(screen.getByText("Base")).toBeInTheDocument());
    expect(screen.getByText("main")).toBeInTheDocument();
    expect(mocks.getGithubPrByPath).toHaveBeenCalledWith(
      "/home/dev/.codemux/worktrees/repo/feature-19",
      172,
    );
  });

  it("omits the Base row when the PR detail fetch fails", async () => {
    mocks.workspace = makeWorkspace({ pr_number: 172, pr_state: "open" });
    mocks.getGithubPrByPath.mockRejectedValue(new Error("network"));
    const user = userEvent.setup();
    render(<WorkspaceStatusCluster />);
    await user.click(screen.getByRole("button", { name: "Workspace details" }));
    await waitFor(() => expect(mocks.getGithubPrByPath).toHaveBeenCalled());
    expect(screen.queryByText("Base")).not.toBeInTheDocument();
  });

  it("shows device name for a remote workspace's Location row", async () => {
    mocks.workspace = makeWorkspace({ host_id: 7, git_behind: 1 });
    mocks.hosts = [{ id: 7, name: "pandora" }];
    const user = userEvent.setup();
    render(<WorkspaceStatusCluster />);
    await user.click(screen.getByRole("button", { name: "Workspace details" }));
    expect(screen.getByText("pandora")).toBeInTheDocument();
  });

  it("footer shows View PR and Sync only when applicable, and Sync calls gitPullChanges", async () => {
    mocks.workspace = makeWorkspace({
      git_behind: 5,
      pr_number: 172,
      pr_state: "open",
      pr_url: "https://github.com/org/repo/pull/172",
    });
    const user = userEvent.setup();
    render(<WorkspaceStatusCluster />);
    await user.click(screen.getByRole("button", { name: "Workspace details" }));
    expect(
      screen.getByRole("button", { name: "View PR #172" }),
    ).toBeInTheDocument();
    const syncBtn = screen.getByRole("button", { name: "Sync ↓5" });
    await user.click(syncBtn);
    expect(mocks.gitPullChanges).toHaveBeenCalledWith(
      "/home/dev/.codemux/worktrees/repo/feature-19",
    );
  });

  it("omits the Sync button when not behind, and the View PR button when there is no PR url", async () => {
    mocks.workspace = makeWorkspace({ git_behind: 0, pr_state: null });
    const user = userEvent.setup();
    render(<WorkspaceStatusCluster />);
    await user.click(screen.getByRole("button", { name: "Workspace details" }));
    expect(screen.queryByText(/^Sync/)).not.toBeInTheDocument();
    expect(screen.queryByText(/^View PR/)).not.toBeInTheDocument();
  });

  it("shows a Sync-failed toast when the pull rejects", async () => {
    mocks.workspace = makeWorkspace({ git_behind: 3 });
    mocks.gitPullChanges.mockRejectedValue(new Error("offline"));
    const user = userEvent.setup();
    render(<WorkspaceStatusCluster />);
    await user.click(screen.getByRole("button", { name: "Workspace details" }));
    await user.click(screen.getByRole("button", { name: "Sync ↓3" }));
    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith(
        expect.stringContaining("Sync failed"),
      ),
    );
  });

  // ── GUI-mode background browser indicator ──
  // Retained from the retired bottom context bar — same shared
  // `BackgroundBrowserIndicator` chip, same session predicate.

  it("shows the background-browser indicator for a live detached session, and opens the peek on click", async () => {
    mocks.workspace = makeWorkspace();
    mocks.agentBrowserSessions = [makeBackgroundSession()];
    render(<WorkspaceStatusCluster />);
    const indicator = screen.getByRole("button", {
      name: /Browser running in background/,
    });
    expect(indicator).toBeInTheDocument();
    expect(useBrowserPeekStore.getState().isOpen("ws-1")).toBe(false);
    await userEvent.click(indicator);
    expect(useBrowserPeekStore.getState().isOpen("ws-1")).toBe(true);
  });

  it("hides the indicator when there is no agent browser session", () => {
    mocks.workspace = makeWorkspace();
    render(<WorkspaceStatusCluster />);
    expect(
      screen.queryByRole("button", { name: /Browser running in background/ }),
    ).not.toBeInTheDocument();
  });

  it("hides the indicator once the session is attached to a pane (no longer background)", () => {
    mocks.workspace = makeWorkspace();
    mocks.agentBrowserSessions = [
      makeBackgroundSession({ pane_id: "pane-1", browser_id: "browser-1" }),
    ];
    render(<WorkspaceStatusCluster />);
    expect(
      screen.queryByRole("button", { name: /Browser running in background/ }),
    ).not.toBeInTheDocument();
  });

  it("hides the indicator for an inactive session", () => {
    mocks.workspace = makeWorkspace();
    mocks.agentBrowserSessions = [makeBackgroundSession({ is_active: false })];
    render(<WorkspaceStatusCluster />);
    expect(
      screen.queryByRole("button", { name: /Browser running in background/ }),
    ).not.toBeInTheDocument();
  });

  it("renders the indicator alone (no git chips / details button) for a git-less workspace with a live background session", () => {
    mocks.workspace = makeWorkspace({ git_branch: null });
    mocks.agentBrowserSessions = [makeBackgroundSession()];
    const { container } = render(<WorkspaceStatusCluster />);
    expect(container).not.toBeEmptyDOMElement();
    expect(
      screen.getByRole("button", { name: /Browser running in background/ }),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Workspace details" }),
    ).not.toBeInTheDocument();
  });

  it("keeps the git chips unaffected when the indicator renders alongside them", () => {
    mocks.workspace = makeWorkspace({
      git_behind: 5,
      pr_number: 172,
      pr_state: "open",
      pr_url: "https://github.com/org/repo/pull/172",
    });
    mocks.agentBrowserSessions = [makeBackgroundSession()];
    render(<WorkspaceStatusCluster />);
    expect(
      screen.getByRole("button", { name: /Browser running in background/ }),
    ).toBeInTheDocument();
    expect(screen.getByText("5")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /Open PR #172 on GitHub/ }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Workspace details" }),
    ).toBeInTheDocument();
  });

  // ── Linked-issue chip ──
  // Retained from the retired bottom context bar — same shared `IssueDetailPopover`
  // (chip variant, opening upward), so a thread's linked issue stays
  // visible on the Context Row (regression from PR #144).

  const LINKED_ISSUE = {
    number: 146,
    title: "Context Row loses the linked-issue chip",
    state: "Open" as const,
    labels: [],
  };

  it("renders the linked-issue chip and opens the detail popover upward", async () => {
    mocks.workspace = makeWorkspace({ linked_issue: LINKED_ISSUE });
    render(<WorkspaceStatusCluster />);
    const chip = screen.getByText("Issue #146").closest("button")!;
    expect(chip).toBeInTheDocument();
    await userEvent.click(chip);
    const content = await waitFor(() => {
      const el = document.querySelector(
        "[data-testid='issue-detail-content']",
      );
      expect(el).not.toBeNull();
      return el as HTMLElement;
    });
    // The popover opens upward (`side="top"`, mirroring the old bar).
    expect(content.closest("[data-side]")).toHaveAttribute("data-side", "top");
    expect(mocks.getGithubIssue).toHaveBeenCalledWith("ws-1", 146);
  });

  it("renders the issue chip alone (no git chips / details button) for a branch-less workspace with a linked issue", () => {
    mocks.workspace = makeWorkspace({
      git_branch: null,
      linked_issue: LINKED_ISSUE,
    });
    const { container } = render(<WorkspaceStatusCluster />);
    expect(container).not.toBeEmptyDOMElement();
    expect(screen.getByText("Issue #146")).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "Workspace details" }),
    ).not.toBeInTheDocument();
  });

  it("renders no issue chip when the workspace has no linked issue", () => {
    mocks.workspace = makeWorkspace({ linked_issue: null });
    render(<WorkspaceStatusCluster />);
    expect(screen.queryByText(/^Issue #/)).not.toBeInTheDocument();
  });

  it("shows the Issue row in the details popover when a linked issue is present", async () => {
    mocks.workspace = makeWorkspace({ linked_issue: LINKED_ISSUE });
    const user = userEvent.setup();
    render(<WorkspaceStatusCluster />);
    await user.click(screen.getByRole("button", { name: "Workspace details" }));
    expect(screen.getByText("Issue")).toBeInTheDocument();
    expect(screen.getByText("#146 · Open")).toBeInTheDocument();
  });

  it("omits the Issue row from the details popover without a linked issue", async () => {
    mocks.workspace = makeWorkspace({ linked_issue: null });
    const user = userEvent.setup();
    render(<WorkspaceStatusCluster />);
    await user.click(screen.getByRole("button", { name: "Workspace details" }));
    expect(screen.queryByText("Issue")).not.toBeInTheDocument();
  });
});
