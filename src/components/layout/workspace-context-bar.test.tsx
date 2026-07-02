/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { WorkspaceSnapshot } from "@/tauri/types";

// ── Mocks ──
//
// `vi.mock()` factories are hoisted above `import`s, so the mutable
// state they close over is created via `vi.hoisted`.
const mocks = vi.hoisted(() => ({
  workspace: null as WorkspaceSnapshot | null,
  lazyEnabled: false,
  activeDraftId: null as string | null,
  onboardingProjectDir: null as string | null,
  hosts: [] as Array<{ id: number; name: string }>,
  openUrl: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@/stores/app-store", () => ({
  useActiveWorkspace: () => mocks.workspace,
}));
vi.mock("@/stores/feature-flags", () => ({
  useFeatureFlags: (sel: (s: Record<string, unknown>) => unknown) =>
    sel({ enableLazyWorkspaceCreation: mocks.lazyEnabled }),
}));
vi.mock("@/stores/chat-draft-store", () => ({
  useChatDraftStore: (sel: (s: Record<string, unknown>) => unknown) =>
    sel({ activeDraftId: mocks.activeDraftId }),
}));
vi.mock("@/stores/ui-store", () => ({
  useUIStore: (sel: (s: Record<string, unknown>) => unknown) =>
    sel({ onboardingProjectDir: mocks.onboardingProjectDir }),
}));
vi.mock("@/stores/hosts-store", () => ({
  useHosts: () => mocks.hosts,
}));
vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: (...args: unknown[]) => mocks.openUrl(...args),
}));
// IssueDetailPopover fetches the full issue lazily on open; the bar
// tests only assert the trigger chip, so a null resolve is enough.
vi.mock("@/tauri/commands", () => ({
  getGithubIssue: vi.fn().mockResolvedValue(null),
}));

// Late import so the mocks above apply.
import { WorkspaceContextBar } from "./workspace-context-bar";

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

beforeEach(() => {
  mocks.workspace = null;
  mocks.lazyEnabled = false;
  mocks.activeDraftId = null;
  mocks.onboardingProjectDir = null;
  mocks.hosts = [];
  mocks.openUrl.mockClear();
});

afterEach(cleanup);

describe("WorkspaceContextBar", () => {
  it("renders nothing when there is no active workspace", () => {
    const { container } = render(<WorkspaceContextBar />);
    expect(container).toBeEmptyDOMElement();
  });

  it("renders nothing when the workspace has no git context", () => {
    mocks.workspace = makeWorkspace({
      git_branch: null,
      pr_state: null,
      linked_issue: null,
    });
    const { container } = render(<WorkspaceContextBar />);
    expect(container).toBeEmptyDOMElement();
  });

  it("renders nothing while a lazy-creation chat draft is active", () => {
    mocks.workspace = makeWorkspace();
    mocks.lazyEnabled = true;
    mocks.activeDraftId = "draft-1";
    const { container } = render(<WorkspaceContextBar />);
    expect(container).toBeEmptyDOMElement();
  });

  it("renders nothing while onboarding covers the active workspace", () => {
    mocks.workspace = makeWorkspace();
    mocks.onboardingProjectDir = "/home/dev/projects/repo";
    const { container } = render(<WorkspaceContextBar />);
    expect(container).toBeEmptyDOMElement();
  });

  it("shows branch, worktree kind, and 'This device' for a local worktree", () => {
    mocks.workspace = makeWorkspace();
    render(<WorkspaceContextBar />);
    expect(screen.getByText("feature/19-cloud-push")).toBeInTheDocument();
    expect(screen.getByText("worktree")).toBeInTheDocument();
    expect(screen.getByText("This device")).toBeInTheDocument();
  });

  it("labels a repo-root checkout as 'repo root'", () => {
    mocks.workspace = makeWorkspace({
      git_branch: "main",
      worktree_path: null,
      workspace_kind: "main",
    });
    render(<WorkspaceContextBar />);
    expect(screen.getByText("repo root")).toBeInTheDocument();
  });

  it("shows ahead/behind, diff counters, and the changed-file count", () => {
    mocks.workspace = makeWorkspace({
      git_ahead: 2,
      git_behind: 1,
      git_additions: 214,
      git_deletions: 37,
      git_changed_files: 6,
    });
    render(<WorkspaceContextBar />);
    expect(screen.getByText("↑2")).toBeInTheDocument();
    expect(screen.getByText("↓1")).toBeInTheDocument();
    expect(screen.getByText("+214")).toBeInTheDocument();
    expect(screen.getByText("−37")).toBeInTheDocument();
    expect(screen.getByText(/6 files/)).toBeInTheDocument();
  });

  it("singularizes a one-file change", () => {
    mocks.workspace = makeWorkspace({ git_changed_files: 1 });
    render(<WorkspaceContextBar />);
    expect(screen.getByText(/1 file$/)).toBeInTheDocument();
  });

  it("hides zero-valued counters", () => {
    mocks.workspace = makeWorkspace();
    render(<WorkspaceContextBar />);
    expect(screen.queryByText(/↑/)).not.toBeInTheDocument();
    expect(screen.queryByText(/↓/)).not.toBeInTheDocument();
    expect(screen.queryByText(/files/)).not.toBeInTheDocument();
  });

  it("opens the PR on GitHub from the PR chip", async () => {
    mocks.workspace = makeWorkspace({
      pr_number: 19,
      pr_state: "open",
      pr_url: "https://github.com/org/repo/pull/19",
    });
    render(<WorkspaceContextBar />);
    const chip = screen.getByRole("button", { name: /Open PR #19 on GitHub/ });
    expect(chip).toHaveTextContent("PR #19 · Open");
    await userEvent.click(chip);
    expect(mocks.openUrl).toHaveBeenCalledWith(
      "https://github.com/org/repo/pull/19",
    );
  });

  it("disables the PR chip when there is no PR URL", () => {
    mocks.workspace = makeWorkspace({
      pr_number: 19,
      pr_state: "draft",
      pr_url: null,
    });
    render(<WorkspaceContextBar />);
    expect(
      screen.getByRole("button", { name: /Open PR #19 on GitHub/ }),
    ).toBeDisabled();
  });

  it("renders the linked-issue chip", () => {
    mocks.workspace = makeWorkspace({
      linked_issue: {
        number: 40,
        title: "mock Tauri runtime",
        state: "Open",
        labels: [],
      },
    });
    render(<WorkspaceContextBar />);
    expect(screen.getByText("Issue #40")).toBeInTheDocument();
  });

  it("shows the host name for a remote workspace", () => {
    mocks.workspace = makeWorkspace({ host_id: 7 });
    mocks.hosts = [{ id: 7, name: "pandora" }];
    render(<WorkspaceContextBar />);
    expect(screen.getByText("pandora")).toBeInTheDocument();
    expect(screen.queryByText("This device")).not.toBeInTheDocument();
  });
});
