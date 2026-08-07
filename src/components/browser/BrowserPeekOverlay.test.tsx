/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { TooltipProvider } from "@/components/ui/tooltip";
import { useAppStore } from "@/stores/app-store";
import { useBrowserPeekStore } from "@/stores/browser-peek-store";
import { useUIStore } from "@/stores/ui-store";
import type { AgentBrowserSession, AppStateSnapshot, WorkspaceSnapshot } from "@/tauri/types";

// The overlay's job is positioning/promote/close chrome — the actual
// screencast stream is BrowserPane's concern (WebSocket + canvas, not
// testable in jsdom without heavy mocking). Stub it to a sentinel so we
// can assert the props the overlay hands it.
vi.mock("@/components/browser/BrowserPane", () => ({
  BrowserPane: (props: Record<string, unknown>) => (
    <div
      data-testid="browser-pane-stub"
      data-browser-id={props.browserId as string}
      data-workspace-id={props.workspaceId as string}
      data-hide-toolbar={String(props.hideToolbar)}
    />
  ),
}));

const mocks = vi.hoisted(() => ({
  guiChrome: true,
  dockBrowserInRightPanel: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@/hooks/use-gui-chrome", () => ({
  useGuiChrome: () => mocks.guiChrome,
}));

vi.mock("@/tauri/commands", () => ({
  dockBrowserInRightPanel: (...a: unknown[]) =>
    mocks.dockBrowserInRightPanel(...a),
}));

import { BrowserPeekOverlay } from "./BrowserPeekOverlay";

function makeWorkspace(overrides: Partial<WorkspaceSnapshot> = {}): WorkspaceSnapshot {
  return {
    workspace_id: "ws-1",
    title: "Test",
    workspace_type: "standard",
    cwd: "/path/to/project",
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
    active_surface_id: "ws-1-surface",
    surfaces: [
      {
        surface_id: "ws-1-surface",
        title: "Main",
        active_pane_id: "pane-1",
        root: { kind: "terminal", pane_id: "pane-1", session_id: "sess-1", title: "Terminal" },
      },
    ],
    ...overrides,
  };
}

function makeSession(overrides: Partial<AgentBrowserSession> = {}): AgentBrowserSession {
  return {
    session_id: "abs-1",
    workspace_id: "ws-1",
    cli_session_name: "ws-abc123",
    stream_url: "ws://localhost:9223",
    current_url: "https://example.com/dashboard",
    is_active: true,
    pane_id: null,
    browser_id: null,
    user_dismissed: false,
    ...overrides,
  };
}

function setAppState(sessions: AgentBrowserSession[] = [], workspaceOverrides: Partial<WorkspaceSnapshot> = {}) {
  const ws = makeWorkspace(workspaceOverrides);
  useAppStore.setState({
    appState: {
      schema_version: 1,
      active_workspace_id: ws.workspace_id,
      workspaces: [ws],
      terminal_sessions: [],
      browser_sessions: [],
      agent_browser_sessions: sessions,
      notifications: [],
      detected_ports: [],
      pane_statuses: {},
      persistence: {
        schema_version: 1,
        stores_layout_metadata: true,
        stores_terminal_metadata: true,
        stores_live_process_state: false,
      },
      config: {} as AppStateSnapshot["config"],
    },
  });
}

function renderOverlay() {
  return render(
    <TooltipProvider>
      <BrowserPeekOverlay />
    </TooltipProvider>,
  );
}

beforeEach(() => {
  mocks.guiChrome = true;
  mocks.dockBrowserInRightPanel.mockClear();
  useUIStore.setState({ rightPanelTabs: {}, rightPanelPanes: {} });
  useBrowserPeekStore.setState({ openWorkspaceId: null });
});

afterEach(() => {
  cleanup();
  useAppStore.setState({ appState: null });
});

describe("BrowserPeekOverlay", () => {
  it("renders nothing when the peek isn't open", () => {
    setAppState([makeSession()]);
    const { container } = renderOverlay();
    expect(container).toBeEmptyDOMElement();
  });

  it("renders nothing when GUI chrome doesn't apply, even if opened", () => {
    mocks.guiChrome = false;
    setAppState([makeSession()]);
    useBrowserPeekStore.getState().open("ws-1");
    const { container } = renderOverlay();
    expect(container).toBeEmptyDOMElement();
  });

  it("renders nothing when there is no live background session", () => {
    setAppState([]);
    useBrowserPeekStore.getState().open("ws-1");
    const { container } = renderOverlay();
    expect(container).toBeEmptyDOMElement();
  });

  it("renders the panel with the session URL and stream when open with a live background session", () => {
    setAppState([makeSession()]);
    useBrowserPeekStore.getState().open("ws-1");
    renderOverlay();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.getByText("https://example.com/dashboard")).toBeInTheDocument();
    const stub = screen.getByTestId("browser-pane-stub");
    expect(stub.dataset.browserId).toBe("ws-abc123");
    expect(stub.dataset.workspaceId).toBe("ws-1");
    expect(stub.dataset.hideToolbar).toBe("true");
  });

  it("closes on the close button", async () => {
    setAppState([makeSession()]);
    useBrowserPeekStore.getState().open("ws-1");
    renderOverlay();
    await userEvent.click(screen.getByRole("button", { name: "Close preview" }));
    expect(useBrowserPeekStore.getState().isOpen("ws-1")).toBe(false);
  });

  it("closes on Escape", () => {
    setAppState([makeSession()]);
    useBrowserPeekStore.getState().open("ws-1");
    renderOverlay();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(useBrowserPeekStore.getState().isOpen("ws-1")).toBe(false);
  });

  // Promote docks the session into the right-panel deck rather than
  // splitting a pane: the deck is the browser's one persistent home, so the
  // peek graduates into it instead of creating a second place to live.
  it("promotes into the right-panel deck and closes the peek", async () => {
    setAppState([makeSession()]);
    useBrowserPeekStore.getState().open("ws-1");
    renderOverlay();
    await userEvent.click(
      screen.getByRole("button", { name: "Open in side panel" }),
    );
    expect(mocks.dockBrowserInRightPanel).toHaveBeenCalledWith("ws-1");
    expect(useUIStore.getState().getRightPanelTab("ws-1")).toBe("browser");
    await vi.waitFor(() => {
      expect(useBrowserPeekStore.getState().isOpen("ws-1")).toBe(false);
    });
  });

  // A session the user can already see in the deck is not a "background"
  // session — offering to reveal it would be offering a second copy.
  it("does not render for a session docked in the right panel", () => {
    setAppState([makeSession({ right_panel_docked: true })]);
    useBrowserPeekStore.getState().open("ws-1");
    renderOverlay();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("closes the peek when the active workspace changes (no unprompted re-open on return)", () => {
    setAppState([makeSession()]);
    useBrowserPeekStore.getState().open("ws-1");
    renderOverlay();
    expect(screen.getByRole("dialog")).toBeInTheDocument();

    // Switch A → B: the overlay's effect must clear the stale open state.
    act(() => {
      setAppState([makeSession()], { workspace_id: "ws-2" });
    });
    expect(useBrowserPeekStore.getState().openWorkspaceId).toBeNull();
    expect(screen.queryByRole("dialog")).toBeNull();

    // Back to A: the peek must stay closed until explicitly re-opened.
    act(() => {
      setAppState([makeSession()]);
    });
    expect(useBrowserPeekStore.getState().isOpen("ws-1")).toBe(false);
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
