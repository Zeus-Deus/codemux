/// <reference types="@testing-library/jest-dom/vitest" />
import { memo } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type {
  AgentBrowserSession,
  AppStateSnapshot,
  PaneNodeSnapshot,
  SurfaceSnapshot,
  WorkspaceSnapshot,
} from "@/tauri/types";

// xterm can't mount in jsdom — stub the leaf, keep the real PaneNode header.
vi.mock("@/components/terminal/TerminalPane", () => ({
  TerminalPane: memo(({ sessionId }: { sessionId: string }) => (
    <div data-testid={`term-${sessionId}`} />
  )),
}));
vi.mock("@/components/browser/BrowserPane", () => ({
  BrowserPane: memo(() => <div />),
}));
vi.mock("@/components/chat/AgentChatPane", () => ({
  AgentChatPane: memo(() => <div />),
}));
vi.mock("@/components/chat/AgentChatPaneHeader", () => ({
  AgentChatPaneHeader: memo(() => <div />),
}));
vi.mock("@/tauri/commands", () => ({
  splitPane: vi.fn(),
  closePane: vi.fn(),
  activatePane: vi.fn(),
  resizeSplit: vi.fn(),
  swapPanes: vi.fn(),
}));

import { PaneNode } from "./PaneNode";
import { useAppStore } from "@/stores/app-store";
import { useBrowserPeekStore } from "@/stores/browser-peek-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useTerminalCwdStore } from "@/stores/terminal-cwd-store";

const WS_CWD = "/home/zeus/projects/codemux";

function termPane(title: string): PaneNodeSnapshot {
  return {
    kind: "terminal",
    pane_id: "p1",
    session_id: "s1",
    title,
  };
}

function makeSurface(root: PaneNodeSnapshot): SurfaceSnapshot {
  return { surface_id: "sf", title: "", root, active_pane_id: "p1" };
}

function makeWs(root: PaneNodeSnapshot): WorkspaceSnapshot {
  return {
    workspace_id: "ws-1",
    title: "codemux",
    workspace_type: "standard",
    cwd: WS_CWD,
    git_branch: "main",
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    notification_count: 0,
    notifications_muted: false,
    latest_agent_state: null,
    worktree_path: null,
    project_root: null,
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    tabs: [],
    active_tab_id: "",
    active_surface_id: "sf",
    surfaces: [makeSurface(root)],
  } as unknown as WorkspaceSnapshot;
}

function makeAppState(
  root: PaneNodeSnapshot,
  agentBrowserSessions: AgentBrowserSession[] = [],
): AppStateSnapshot {
  return {
    schema_version: 1,
    active_workspace_id: "ws-1",
    workspaces: [makeWs(root)],
    terminal_sessions: [],
    browser_sessions: [],
    agent_browser_sessions: agentBrowserSessions,
    notifications: [],
    detected_ports: [],
    pane_statuses: {},
  } as unknown as AppStateSnapshot;
}

function mount(
  title = "Terminal",
  opts: {
    isSurfaceRoot?: boolean;
    agentBrowserSessions?: AgentBrowserSession[];
    activePaneId?: string;
  } = {},
) {
  const {
    isSurfaceRoot = false,
    agentBrowserSessions = [],
    activePaneId = "p1",
  } = opts;
  const root = termPane(title);
  act(() => {
    useAppStore.setState({
      appState: makeAppState(root, agentBrowserSessions),
      homeDir: "/home/zeus",
    });
  });
  return render(
    <PaneNode
      node={root}
      activePaneId={activePaneId}
      visible={true}
      isSurfaceRoot={isSurfaceRoot}
    />,
  );
}

/** The cwd hint element, or null when the header isn't showing one. */
function hint() {
  return document.querySelector("[data-pane-cwd]");
}

beforeEach(() => {
  act(() => {
    useTerminalCwdStore.setState({ cwds: {} });
    useBrowserPeekStore.setState({ openWorkspaceId: null });
    useFeatureFlags.setState({
      enableAgentChat: false,
      enableLazyWorkspaceCreation: false,
    });
  });
});

afterEach(() => {
  cleanup();
  act(() => {
    useAppStore.setState({ appState: null, homeDir: null });
    useBrowserPeekStore.setState({ openWorkspaceId: null });
    useFeatureFlags.setState({
      enableAgentChat: false,
      enableLazyWorkspaceCreation: false,
    });
  });
});

describe("terminal pane header cwd hint", () => {
  it("does not recreate a second full-width header for a sole root terminal", () => {
    mount("Terminal", { isSurfaceRoot: true });

    const chrome = document.querySelector("[data-terminal-pane-chrome]");
    expect(chrome).not.toHaveClass("border-b", "bg-card", "bg-background");
    expect(document.querySelector("[data-terminal-pane-context]")).toBeNull();
    expect(screen.queryByText("Terminal")).not.toBeInTheDocument();
    expect(document.querySelector("[data-terminal-pane-actions]")).not.toBeNull();
  });

  it("shows only useful cwd context in a sole root terminal", () => {
    mount("Terminal", { isSurfaceRoot: true });
    act(() => {
      useTerminalCwdStore
        .getState()
        .setCwd("s1", `${WS_CWD}/src-tauri`, "osc7");
    });

    expect(hint()).toHaveTextContent("src-tauri");
    expect(screen.queryByText("Terminal")).not.toBeInTheDocument();
  });

  it("shows only the title when no cwd is known yet", () => {
    mount();
    expect(screen.getByText("Terminal")).toBeInTheDocument();
    expect(hint()).toBeNull();
  });

  it("stays quiet while the shell sits at the workspace root", () => {
    mount();
    act(() => {
      useTerminalCwdStore.getState().setCwd("s1", WS_CWD, "osc7");
    });
    expect(hint()).toBeNull();
  });

  it("shows the relative path once the shell moves into a subdirectory", () => {
    mount();
    act(() => {
      useTerminalCwdStore
        .getState()
        .setCwd("s1", `${WS_CWD}/src-tauri`, "osc7");
    });
    expect(hint()).toHaveTextContent("src-tauri");
    // Split panes keep their local identity alongside the useful context.
    expect(screen.getByText("Terminal")).toBeInTheDocument();
  });

  it("disappears again when the shell returns to the root", () => {
    mount();
    act(() => {
      useTerminalCwdStore.getState().setCwd("s1", `${WS_CWD}/src`, "osc7");
    });
    expect(hint()).not.toBeNull();
    act(() => {
      useTerminalCwdStore.getState().setCwd("s1", WS_CWD, "osc7");
    });
    expect(hint()).toBeNull();
  });

  it("contracts an out-of-workspace path against $HOME", () => {
    mount();
    act(() => {
      useTerminalCwdStore.getState().setCwd("s1", "/home/zeus/dotfiles", "osc7");
    });
    expect(hint()).toHaveTextContent("~/dotfiles");
  });

  it("carries the untrimmed path on the tooltip when the label elides", () => {
    mount();
    const deep = `${WS_CWD}/src-tauri/src/pty_daemon`;
    act(() => {
      useTerminalCwdStore.getState().setCwd("s1", deep, "osc7");
    });
    expect(hint()).toHaveTextContent("…/src/pty_daemon");
    expect(hint()).toHaveAttribute("title", deep);
  });

  it("keeps the preset icon working — the title string is not replaced", () => {
    // `PRESET_TITLE_TO_ICON` keys off the exact title, so appending the cwd
    // rather than overwriting the title is load-bearing for preset panes.
    const { container } = mount("Claude Code");
    act(() => {
      useTerminalCwdStore.getState().setCwd("s1", `${WS_CWD}/src`, "osc7");
    });
    expect(screen.getByText("Claude Code")).toBeInTheDocument();
    expect(container.querySelector("svg, img")).not.toBeNull();
    expect(hint()).toHaveTextContent("src");
  });
});

describe("terminal pane header background browser", () => {
  const liveSession: AgentBrowserSession = {
    session_id: "browser-session-1",
    workspace_id: "ws-1",
    cli_session_name: "ws-browser",
    stream_url: "ws://localhost:9223",
    current_url: "https://example.com",
    is_active: true,
    pane_id: null,
    browser_id: null,
    user_dismissed: false,
  };

  it("surfaces a live detached browser in the active terminal header and opens the peek", async () => {
    act(() => {
      useFeatureFlags.setState({ enableAgentChat: true });
    });
    mount("Terminal", { agentBrowserSessions: [liveSession] });

    const button = screen.getByRole("button", {
      name: /Browser running in background/,
    });
    const actions = document.querySelector("[data-terminal-pane-actions]");
    const closePane = screen.getByRole("button", { name: "Close pane" });
    expect(button).toBeInTheDocument();
    expect(button).not.toHaveTextContent("Browser");
    expect(actions).toContainElement(button);
    expect(actions?.lastElementChild).toBe(closePane);
    expect(button.nextElementSibling).toBe(
      screen.getByRole("button", { name: "Split right" }),
    );
    expect(useBrowserPeekStore.getState().isOpen("ws-1")).toBe(false);

    await userEvent.click(button);
    expect(useBrowserPeekStore.getState().isOpen("ws-1")).toBe(true);
  });

  it("does not show the header control after the browser is attached to a pane", () => {
    act(() => {
      useFeatureFlags.setState({ enableAgentChat: true });
    });
    mount("Terminal", {
      agentBrowserSessions: [
        { ...liveSession, pane_id: "browser-pane-1", browser_id: "browser-1" },
      ],
    });

    expect(
      screen.queryByRole("button", {
        name: /Browser running in background/,
      }),
    ).not.toBeInTheDocument();
  });

  it("does not show the header control when GUI chrome is disabled", () => {
    mount("Terminal", { agentBrowserSessions: [liveSession] });

    expect(
      screen.queryByRole("button", {
        name: /Browser running in background/,
      }),
    ).not.toBeInTheDocument();
  });

  it("does not show the header control on an inactive terminal", () => {
    act(() => {
      useFeatureFlags.setState({ enableAgentChat: true });
    });
    mount("Terminal", { agentBrowserSessions: [liveSession], activePaneId: "another-pane" });

    expect(
      screen.queryByRole("button", {
        name: /Browser running in background/,
      }),
    ).not.toBeInTheDocument();
  });
});
