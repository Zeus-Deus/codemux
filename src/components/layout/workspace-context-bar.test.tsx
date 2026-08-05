/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, cleanup } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { AgentBrowserSession, WorkspaceSnapshot } from "@/tauri/types";

// ── Mocks ──
//
// `vi.mock()` factories are hoisted above `import`s, so the mutable
// state they close over is created via `vi.hoisted`.
const mocks = vi.hoisted(() => ({
  workspace: null as WorkspaceSnapshot | null,
  lazyEnabled: false,
  enableAgentChat: false,
  activeDraftId: null as string | null,
  onboardingProjectDir: null as string | null,
  hosts: [] as Array<{ id: number; name: string }>,
  openUrl: vi.fn().mockResolvedValue(undefined),
  agentBrowserSessions: [] as AgentBrowserSession[],
  agentChatPaneActive: false,
}));

vi.mock("@/stores/app-store", () => ({
  useActiveWorkspace: () => mocks.workspace,
  useAppStore: (sel: (s: Record<string, unknown>) => unknown) =>
    sel({
      appState: { agent_browser_sessions: mocks.agentBrowserSessions },
    }),
}));
vi.mock("@/stores/feature-flags", () => ({
  useFeatureFlags: (sel: (s: Record<string, unknown>) => unknown) =>
    sel({
      enableLazyWorkspaceCreation: mocks.lazyEnabled,
      enableAgentChat: mocks.enableAgentChat,
    }),
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
vi.mock("@/hooks/use-gui-chrome", () => ({
  useAgentChatPaneActive: () => mocks.agentChatPaneActive,
}));
vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: (...args: unknown[]) => mocks.openUrl(...args),
}));
// IssueDetailPopover fetches the full issue lazily on open; the bar
// tests only assert the trigger chip, so a null resolve is enough.
// initGitRepo / refreshWorkspaceGitInfo back the non-git "Initialize
// Git" affordance (use-initialize-git.ts).
vi.mock("@/tauri/commands", () => ({
  getGithubIssue: vi.fn().mockResolvedValue(null),
  initGitRepo: vi.fn().mockResolvedValue("/home/dev/projects/scratch"),
  refreshWorkspaceGitInfo: vi.fn().mockResolvedValue(undefined),
}));

// Late import so the mocks above apply.
import { WorkspaceContextBar } from "./workspace-context-bar";
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
  mocks.lazyEnabled = false;
  mocks.enableAgentChat = false;
  mocks.activeDraftId = null;
  mocks.onboardingProjectDir = null;
  mocks.hosts = [];
  mocks.openUrl.mockClear();
  mocks.agentBrowserSessions = [];
  mocks.agentChatPaneActive = false;
  useBrowserPeekStore.setState({ openWorkspaceId: null });
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

  it("shows the no-git state + Initialize Git for a local non-git project workspace", () => {
    mocks.workspace = makeWorkspace({
      is_git: false,
      git_branch: null,
      worktree_path: null,
    });
    render(<WorkspaceContextBar />);
    expect(screen.getByText("Not a git repository")).toBeInTheDocument();
    expect(
      screen.getByRole("button", {
        name: /initialize a git repository/i,
      }),
    ).toBeInTheDocument();
  });

  it("keeps the no-git affordance off non-standard and host-backed workspaces", () => {
    // Home workspace: not a project — no nudge.
    mocks.workspace = makeWorkspace({
      is_git: false,
      git_branch: null,
      worktree_path: null,
      workspace_type: "home",
    });
    const home = render(<WorkspaceContextBar />);
    expect(home.container).toBeEmptyDOMElement();
    cleanup();

    // Host-backed workspace: local is_git probe is meaningless.
    mocks.workspace = makeWorkspace({
      is_git: false,
      git_branch: null,
      worktree_path: null,
      host_id: 7,
    });
    const hosted = render(<WorkspaceContextBar />);
    expect(hosted.container).toBeEmptyDOMElement();
  });

  it("treats a missing is_git (older snapshot) as git — no affordance flash", () => {
    mocks.workspace = makeWorkspace({
      git_branch: null,
      worktree_path: null,
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

  it("renders nothing when an Agent Chat pane is the active GUI-mode surface (Context Row owns the detail)", () => {
    mocks.workspace = makeWorkspace();
    mocks.agentChatPaneActive = true;
    const { container } = render(<WorkspaceContextBar />);
    expect(container).toBeEmptyDOMElement();
  });

  it("still renders for a non-agent-chat active pane (e.g. a terminal) in GUI mode", () => {
    mocks.workspace = makeWorkspace();
    mocks.agentChatPaneActive = false;
    render(<WorkspaceContextBar />);
    expect(screen.getByText("feature/19-cloud-push")).toBeInTheDocument();
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

  // ── GUI-mode background browser indicator ──
  // (docs/features/browser.md "Background browser in GUI mode")

  it("shows the background-browser indicator in GUI mode with a live background session, and opens the peek on click", async () => {
    mocks.workspace = makeWorkspace();
    mocks.enableAgentChat = true;
    mocks.agentBrowserSessions = [makeBackgroundSession()];
    render(<WorkspaceContextBar />);
    const indicator = screen.getByRole("button", { name: /Browser running in background/ });
    expect(indicator).toBeInTheDocument();
    expect(useBrowserPeekStore.getState().isOpen("ws-1")).toBe(false);
    await userEvent.click(indicator);
    expect(useBrowserPeekStore.getState().isOpen("ws-1")).toBe(true);
  });

  it("hides the background-browser indicator when the Agent Chat beta flag is off (flag-off byte-identical path)", () => {
    mocks.workspace = makeWorkspace();
    mocks.enableAgentChat = false;
    mocks.agentBrowserSessions = [makeBackgroundSession()];
    render(<WorkspaceContextBar />);
    expect(
      screen.queryByRole("button", { name: /Browser running in background/ }),
    ).not.toBeInTheDocument();
  });

  it("hides the background-browser indicator once the session is attached to a pane (no longer background)", () => {
    mocks.workspace = makeWorkspace();
    mocks.enableAgentChat = true;
    mocks.agentBrowserSessions = [
      makeBackgroundSession({ pane_id: "pane-1", browser_id: "browser-1" }),
    ];
    render(<WorkspaceContextBar />);
    expect(
      screen.queryByRole("button", { name: /Browser running in background/ }),
    ).not.toBeInTheDocument();
  });

  it("renders the bar for the browser indicator alone even with no git/PR/issue to report", () => {
    mocks.workspace = makeWorkspace({
      git_branch: null,
      pr_state: null,
      linked_issue: null,
    });
    mocks.enableAgentChat = true;
    mocks.agentBrowserSessions = [makeBackgroundSession()];
    const { container } = render(<WorkspaceContextBar />);
    expect(container).not.toBeEmptyDOMElement();
    expect(
      screen.getByRole("button", { name: /Browser running in background/ }),
    ).toBeInTheDocument();
  });
});
