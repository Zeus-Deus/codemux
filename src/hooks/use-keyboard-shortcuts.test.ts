import { describe, it, expect, beforeEach, vi } from "vitest";

vi.mock("@/tauri/commands", () => ({
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
  cycleWorkspace: vi.fn().mockResolvedValue(undefined),
  splitPane: vi.fn().mockResolvedValue(undefined),
  closePane: vi.fn().mockResolvedValue(undefined),
  createTab: vi.fn().mockResolvedValue(undefined),
  closeTab: vi.fn().mockResolvedValue(undefined),
  activateTab: vi.fn().mockResolvedValue(undefined),
  createEmptyWorkspace: vi.fn().mockResolvedValue("ws-new"),
  agentChatCreatePane: vi.fn().mockResolvedValue("pane-new"),
  runProjectDevCommand: vi.fn().mockResolvedValue(undefined),
  undockBrowserFromRightPanel: vi.fn().mockResolvedValue(undefined),
}));

import { dispatch } from "./use-keyboard-shortcuts";
import { RIGHT_PANEL_EMPTY, useUIStore } from "@/stores/ui-store";
import { useAppStore } from "@/stores/app-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { activateWorkspace, undockBrowserFromRightPanel } from "@/tauri/commands";
import {
  setJumpTargets,
} from "@/components/layout/sidebar-inbox-jump";

// A fake KeyboardEvent — dispatch only uses the second arg when it needs to
// call preventDefault via the caller, not inside dispatch itself, so a stub
// is fine for closeOverlay-path testing.
const FAKE_EVENT = new KeyboardEvent("keydown", { key: "Escape" });

beforeEach(() => {
  useAppStore.setState({ appState: null });
  useUIStore.setState({
    onboardingProjectDir: null,
    hasSeenOnboarding: false,
    showSettings: false,
    showFileSearch: false,
    showContentSearch: false,
    showCommandPalette: false,
    showNewWorkspaceDialog: false,
  });
  // Restore the store's boot state (GUI on) so a case that opts out
  // doesn't leak into the next one.
  const initialFlags = useFeatureFlags.getInitialState();
  useFeatureFlags.setState({
    enableAgentChat: initialFlags.enableAgentChat,
    enableLazyWorkspaceCreation: initialFlags.enableLazyWorkspaceCreation,
  });
  window.localStorage.clear();
});

describe("use-keyboard-shortcuts dispatch — closeOverlay precedence", () => {
  it("returns false when no overlay is open", () => {
    const handled = dispatch("closeOverlay", FAKE_EVENT);
    expect(handled).toBe(false);
  });

  it("returns false for an unknown actionId", () => {
    // Sanity check — dispatch should not swallow unrelated actions even when
    // state that closeOverlay cares about is set.
    useUIStore.setState({ showSettings: true });
    const handled = dispatch("someOtherAction", FAKE_EVENT);
    expect(handled).toBe(false);
    // And settings stays open.
    expect(useUIStore.getState().showSettings).toBe(true);
  });

  describe("onboarding priority", () => {
    it("clears onboardingProjectDir when it's set (the escape-hatch fix)", () => {
      useUIStore.setState({ onboardingProjectDir: "/home/user/myproj" });

      const handled = dispatch("closeOverlay", FAKE_EVENT);

      expect(handled).toBe(true);
      const s = useUIStore.getState();
      expect(s.onboardingProjectDir).toBeNull();
      // And Escape counts as "onboarding seen" so Escape-dismissal doesn't
      // re-arm on the next project open.
      expect(s.hasSeenOnboarding).toBe(true);
    });

    it("onboarding takes precedence over settings", () => {
      useUIStore.setState({
        onboardingProjectDir: "/home/user/myproj",
        showSettings: true,
      });

      dispatch("closeOverlay", FAKE_EVENT);

      const s = useUIStore.getState();
      expect(s.onboardingProjectDir).toBeNull();
      // Settings stays open — only one overlay closes per Escape press.
      expect(s.showSettings).toBe(true);
    });

    it("onboarding takes precedence over all other overlays simultaneously", () => {
      useUIStore.setState({
        onboardingProjectDir: "/home/user/myproj",
        showSettings: true,
        showFileSearch: true,
        showContentSearch: true,
        showCommandPalette: true,
      });

      dispatch("closeOverlay", FAKE_EVENT);

      const s = useUIStore.getState();
      expect(s.onboardingProjectDir).toBeNull();
      expect(s.showSettings).toBe(true);
      expect(s.showFileSearch).toBe(true);
      expect(s.showContentSearch).toBe(true);
      expect(s.showCommandPalette).toBe(true);
    });
  });

  describe("other overlays — ordering preserved", () => {
    it("closes settings when onboarding is not active", () => {
      useUIStore.setState({ showSettings: true });
      const handled = dispatch("closeOverlay", FAKE_EVENT);
      expect(handled).toBe(true);
      expect(useUIStore.getState().showSettings).toBe(false);
    });

    it("closes fileSearch before contentSearch", () => {
      useUIStore.setState({ showFileSearch: true, showContentSearch: true });
      dispatch("closeOverlay", FAKE_EVENT);
      const s = useUIStore.getState();
      expect(s.showFileSearch).toBe(false);
      expect(s.showContentSearch).toBe(true);
    });

    it("closes contentSearch before commandPalette", () => {
      useUIStore.setState({
        showContentSearch: true,
        showCommandPalette: true,
      });
      dispatch("closeOverlay", FAKE_EVENT);
      const s = useUIStore.getState();
      expect(s.showContentSearch).toBe(false);
      expect(s.showCommandPalette).toBe(true);
    });

    it("closes commandPalette when it's the only overlay open", () => {
      useUIStore.setState({ showCommandPalette: true });
      const handled = dispatch("closeOverlay", FAKE_EVENT);
      expect(handled).toBe(true);
      expect(useUIStore.getState().showCommandPalette).toBe(false);
    });
  });

  describe("other dispatch actions still work", () => {
    it("commandPalette toggles on", () => {
      expect(useUIStore.getState().showCommandPalette).toBe(false);
      const handled = dispatch("commandPalette", FAKE_EVENT);
      expect(handled).toBe(true);
      expect(useUIStore.getState().showCommandPalette).toBe(true);
    });

    it("openSettings opens settings", () => {
      const handled = dispatch("openSettings", FAKE_EVENT);
      expect(handled).toBe(true);
      expect(useUIStore.getState().showSettings).toBe(true);
    });

    it("newAgent opens the New Workspace dialog when agent chat is off", () => {
      // With the Agent Chat GUI opted out, the New agent shortcut falls
      // back to the dialog (same as the sidebar + button). The flags
      // store boots ON to match the backend default, so opt out here.
      useFeatureFlags.setState({
        enableAgentChat: false,
        enableLazyWorkspaceCreation: false,
      });
      expect(useUIStore.getState().showNewWorkspaceDialog).toBe(false);
      const handled = dispatch("newAgent", FAKE_EVENT);
      expect(handled).toBe(true);
      expect(useUIStore.getState().showNewWorkspaceDialog).toBe(true);
    });

    it("newAgent works with no active workspace (runs before the appState guard)", () => {
      useAppStore.setState({ appState: null });
      const handled = dispatch("newAgent", FAKE_EVENT);
      expect(handled).toBe(true);
    });

    it("showShortcuts opens settings", () => {
      const handled = dispatch("showShortcuts", FAKE_EVENT);
      expect(handled).toBe(true);
      expect(useUIStore.getState().showSettings).toBe(true);
    });
  });

  describe("workspace jump shortcuts", () => {
    beforeEach(() => {
      vi.mocked(activateWorkspace).mockClear();
      setJumpTargets([]);
    });

    it("activates the Nth visible sidebar-inbox card (runs before the appState guard)", () => {
      setJumpTargets(["ws-a", "ws-b", "ws-c"]);
      const handled = dispatch("workspaceJump2", FAKE_EVENT);
      expect(handled).toBe(true);
      expect(activateWorkspace).toHaveBeenCalledWith("ws-b");
    });

    it("consumes the combo but activates nothing when the slot is empty", () => {
      setJumpTargets(["ws-a"]);
      const handled = dispatch("workspaceJump5", FAKE_EVENT);
      expect(handled).toBe(true);
      expect(activateWorkspace).not.toHaveBeenCalled();
    });
  });
});

// Ctrl+Shift+B is a fourth way to collapse the panel, and it used to write
// the tab straight to `null`. A collapsed panel is not a surface: leaving
// the agent's browser docked would keep the backend believing it is on
// screen, so it would neither split a pane for it nor raise the background
// chip — invisible and unrevealable until the panel came back.
describe("use-keyboard-shortcuts dispatch — toggleRightPanel", () => {
  function seedWorkspace(browserDocked: boolean) {
    useAppStore.setState({
      appState: {
        active_workspace_id: "ws-1",
        workspaces: [{ workspace_id: "ws-1", surfaces: [] }],
        agent_browser_sessions: [
          { workspace_id: "ws-1", right_panel_docked: browserDocked },
        ],
      } as unknown as NonNullable<ReturnType<typeof useAppStore.getState>["appState"]>,
    });
  }

  beforeEach(() => {
    vi.mocked(undockBrowserFromRightPanel).mockClear();
    useUIStore.setState({ rightPanelTabs: {}, rightPanelPanes: {} });
  });

  it("undocks the agent browser when it collapses the panel", () => {
    seedWorkspace(true);
    useUIStore.getState().setRightPanelTab("ws-1", "browser");

    const handled = dispatch("toggleRightPanel", FAKE_EVENT);

    expect(handled).toBe(true);
    expect(undockBrowserFromRightPanel).toHaveBeenCalledWith("ws-1", false);
    expect(useUIStore.getState().getRightPanelTab("ws-1")).toBeNull();
    // Not a dismissal — the tab stays in the deck for the next open.
    expect(useUIStore.getState().getRightPanelPanes("ws-1")).toContain(
      "browser",
    );
  });

  it("opens onto the picker rather than force-opening Files", () => {
    seedWorkspace(false);
    // The user had closed the Files pane; re-opening the panel must not
    // silently undo that dismissal.
    useUIStore.setState({
      rightPanelPanes: { "ws-1": ["changes"] },
      rightPanelDismissedPanes: { "ws-1": ["files"] },
    });

    const handled = dispatch("toggleRightPanel", FAKE_EVENT);

    expect(handled).toBe(true);
    expect(useUIStore.getState().getRightPanelTab("ws-1")).toBe(
      RIGHT_PANEL_EMPTY,
    );
    expect(useUIStore.getState().getRightPanelPanes("ws-1")).toEqual([
      "changes",
    ]);
  });

  it("leaves a browser that is not docked alone", () => {
    seedWorkspace(false);
    useUIStore.getState().setRightPanelTab("ws-1", "files");

    dispatch("toggleRightPanel", FAKE_EVENT);

    expect(undockBrowserFromRightPanel).not.toHaveBeenCalled();
    expect(useUIStore.getState().getRightPanelTab("ws-1")).toBeNull();
  });
});
