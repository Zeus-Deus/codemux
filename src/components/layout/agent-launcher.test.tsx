/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

import type {
  PresetStoreSnapshot,
  TerminalPreset,
  WorkspaceSnapshot,
} from "@/tauri/types";

// ── Mocks ──
const mocks = vi.hoisted(() => ({
  presetStore: null as PresetStoreSnapshot | null,
  agentChatCreatePane: vi.fn().mockResolvedValue("pane-new"),
  applyPreset: vi.fn().mockResolvedValue(undefined),
  createTab: vi.fn().mockResolvedValue("tab-new"),
  createBrowserPane: vi.fn().mockResolvedValue("pane-b"),
  setShowSettings: vi.fn(),
}));

vi.mock("@/hooks/use-preset-store", () => ({
  usePresetStore: () => mocks.presetStore,
}));

vi.mock("@/tauri/commands", () => ({
  agentChatCreatePane: (...a: unknown[]) => mocks.agentChatCreatePane(...a),
  applyPreset: (...a: unknown[]) => mocks.applyPreset(...a),
  createTab: (...a: unknown[]) => mocks.createTab(...a),
  createBrowserPane: (...a: unknown[]) => mocks.createBrowserPane(...a),
}));

vi.mock("@/lib/toast", () => ({
  toast: { error: vi.fn(), success: vi.fn(), info: vi.fn(), warning: vi.fn() },
}));

vi.mock("@/stores/ui-store", () => ({
  useUIStore: Object.assign(vi.fn(), {
    getState: () => ({ setShowSettings: mocks.setShowSettings }),
  }),
}));

import { useTitlebarPinsStore } from "@/stores/titlebar-pins-store";
import { AgentLauncher } from "./agent-launcher";

function mkPreset(p: Partial<TerminalPreset> & { id: string; name: string }): TerminalPreset {
  return {
    description: null,
    commands: [],
    working_directory: null,
    launch_mode: "new_tab",
    icon: null,
    pinned: true,
    is_builtin: true,
    auto_run_on_workspace: false,
    auto_run_on_new_tab: false,
    kind: "cli",
    launch_config: null,
    ...p,
  };
}

function makeWorkspace(): WorkspaceSnapshot {
  return {
    workspace_id: "ws-1",
    title: "demo",
    workspace_type: "standard",
    cwd: "/p",
    git_branch: "main",
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    notification_count: 0,
    latest_agent_state: null,
    worktree_path: null,
    project_root: "/p",
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    notifications_muted: false,
    tabs: [],
    active_tab_id: "tab-1",
    active_surface_id: "surface-1",
    surfaces: [
      {
        surface_id: "surface-1",
        title: "s",
        active_pane_id: "pane-1",
        root: { kind: "terminal", pane_id: "pane-1", session_id: "sess-1", title: "sh" },
      },
    ],
  };
}

function openLauncher() {
  const trigger = screen.getByTestId("agent-launcher-trigger");
  act(() => {
    fireEvent.pointerDown(trigger, { button: 0, pointerType: "mouse" });
    fireEvent.click(trigger);
  });
}

beforeEach(() => {
  mocks.presetStore = {
    bar_visible: true,
    default_preset_id: null,
    presets: [
      mkPreset({ id: "builtin-claude", name: "Claude Code", icon: "claude", pinned: true }),
      mkPreset({ id: "builtin-codex", name: "Codex", icon: "codex", pinned: false }),
      mkPreset({
        id: "builtin-chat-agent",
        name: "Chat Agent",
        icon: "chat-agent",
        kind: "chat_agent",
        pinned: true,
      }),
    ],
  };
  mocks.agentChatCreatePane.mockClear();
  mocks.applyPreset.mockClear();
  mocks.createTab.mockClear();
  mocks.createBrowserPane.mockClear();
  localStorage.clear();
  useTitlebarPinsStore.setState({ pinnedIds: [] });
});

afterEach(() => {
  cleanup();
  localStorage.clear();
  useTitlebarPinsStore.setState({ pinnedIds: [] });
});

describe("AgentLauncher", () => {
  it("lists GUI + CLI sections from the preset snapshot", () => {
    render(<AgentLauncher workspace={makeWorkspace()} />);
    openLauncher();

    expect(screen.getByText("GUI")).toBeInTheDocument();
    expect(screen.getByText("CLI agents")).toBeInTheDocument();
    expect(screen.getByText("Chat Agent")).toBeInTheDocument();
    expect(screen.getByText("Claude Code")).toBeInTheDocument();
    expect(screen.getByText("Codex")).toBeInTheDocument();
    // Panes + Manage presets footer.
    expect(screen.getByText("Terminal")).toBeInTheDocument();
    expect(screen.getByText("Browser")).toBeInTheDocument();
    expect(screen.getByText("Manage presets…")).toBeInTheDocument();
  });

  it("launches a chat pane when a GUI preset is chosen", () => {
    render(<AgentLauncher workspace={makeWorkspace()} />);
    openLauncher();
    act(() => {
      fireEvent.click(screen.getByTestId("launcher-gui-builtin-chat-agent"));
    });
    expect(mocks.agentChatCreatePane).toHaveBeenCalledWith(
      "ws-1",
      "claude",
      null,
      "new_tab",
    );
    expect(mocks.applyPreset).not.toHaveBeenCalled();
  });

  it("applies a CLI preset in a new tab when chosen", () => {
    render(<AgentLauncher workspace={makeWorkspace()} />);
    openLauncher();
    act(() => {
      fireEvent.click(screen.getByTestId("launcher-cli-builtin-claude"));
    });
    expect(mocks.applyPreset).toHaveBeenCalledWith(
      "ws-1",
      "builtin-claude",
      "new_tab",
      null,
      null,
    );
    expect(mocks.agentChatCreatePane).not.toHaveBeenCalled();
  });

  it("splits the pane on Shift+click of a CLI preset", () => {
    render(<AgentLauncher workspace={makeWorkspace()} />);
    openLauncher();
    const item = screen.getByTestId("launcher-cli-builtin-claude");
    act(() => {
      fireEvent.mouseDown(item, { shiftKey: true });
      fireEvent.click(item);
    });
    expect(mocks.applyPreset).toHaveBeenCalledWith(
      "ws-1",
      "builtin-claude",
      "split_pane",
      null,
      null,
    );
  });

  it("creates a Terminal pane from the Panes section", () => {
    render(<AgentLauncher workspace={makeWorkspace()} />);
    openLauncher();
    act(() => {
      fireEvent.click(screen.getByText("Terminal"));
    });
    expect(mocks.createTab).toHaveBeenCalledWith("ws-1", "terminal");
  });
});

// Issue 2 fix — pinned title-bar tiles are opt-in via a hover-revealed pin
// toggle on each GUI/CLI row, backed by `useTitlebarPinsStore` (separate
// from `preset.pinned`, the legacy-PresetBar flag). The toggle must never
// launch the row.
describe("AgentLauncher — titlebar pin toggle", () => {
  it("toggles the titlebar-pins store for a GUI row without launching it", () => {
    render(<AgentLauncher workspace={makeWorkspace()} />);
    openLauncher();
    expect(useTitlebarPinsStore.getState().isTitlebarPinned("builtin-chat-agent")).toBe(false);
    act(() => {
      fireEvent.click(screen.getByTestId("launcher-pin-toggle-builtin-chat-agent"));
    });
    expect(useTitlebarPinsStore.getState().isTitlebarPinned("builtin-chat-agent")).toBe(true);
    expect(mocks.agentChatCreatePane).not.toHaveBeenCalled();
  });

  it("toggles the titlebar-pins store for a CLI row without launching it", () => {
    render(<AgentLauncher workspace={makeWorkspace()} />);
    openLauncher();
    expect(useTitlebarPinsStore.getState().isTitlebarPinned("builtin-claude")).toBe(false);
    act(() => {
      fireEvent.click(screen.getByTestId("launcher-pin-toggle-builtin-claude"));
    });
    expect(useTitlebarPinsStore.getState().isTitlebarPinned("builtin-claude")).toBe(true);
    expect(mocks.applyPreset).not.toHaveBeenCalled();
  });

  it("unpins on a second click, still without launching", () => {
    useTitlebarPinsStore.setState({ pinnedIds: ["builtin-claude"] });
    render(<AgentLauncher workspace={makeWorkspace()} />);
    openLauncher();
    act(() => {
      fireEvent.click(screen.getByTestId("launcher-pin-toggle-builtin-claude"));
    });
    expect(useTitlebarPinsStore.getState().isTitlebarPinned("builtin-claude")).toBe(false);
    expect(mocks.applyPreset).not.toHaveBeenCalled();
  });

  it("shows the pinned aria-label/title once a preset is pinned", () => {
    useTitlebarPinsStore.setState({ pinnedIds: ["builtin-claude"] });
    render(<AgentLauncher workspace={makeWorkspace()} />);
    openLauncher();
    const toggle = screen.getByTestId("launcher-pin-toggle-builtin-claude");
    expect(toggle).toHaveAttribute("aria-label", "Unpin from title bar");
    const unpinned = screen.getByTestId("launcher-pin-toggle-builtin-codex");
    expect(unpinned).toHaveAttribute("aria-label", "Pin to title bar");
  });

  it("does not select the row when Shift-clicking the pin toggle on a CLI row", () => {
    render(<AgentLauncher workspace={makeWorkspace()} />);
    openLauncher();
    const toggle = screen.getByTestId("launcher-pin-toggle-builtin-claude");
    act(() => {
      fireEvent.mouseDown(toggle, { shiftKey: true });
      fireEvent.click(toggle);
    });
    expect(mocks.applyPreset).not.toHaveBeenCalled();
    expect(useTitlebarPinsStore.getState().isTitlebarPinned("builtin-claude")).toBe(true);
  });
});
