import { describe, it, expect, beforeEach, vi } from "vitest";

vi.mock("@/tauri/commands", () => ({
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
  splitPane: vi.fn().mockResolvedValue(undefined),
  closePane: vi.fn().mockResolvedValue(undefined),
  createTab: vi.fn().mockResolvedValue(undefined),
  closeTab: vi.fn().mockResolvedValue(undefined),
  activateTab: vi.fn().mockResolvedValue(undefined),
  activatePane: vi.fn().mockResolvedValue(undefined),
  createEmptyWorkspace: vi.fn().mockResolvedValue("ws-new"),
  agentChatCreatePane: vi.fn().mockResolvedValue("pane-new"),
  runProjectDevCommand: vi.fn().mockResolvedValue(undefined),
  undockBrowserFromRightPanel: vi.fn().mockResolvedValue(undefined),
}));

import { dispatch } from "./use-keyboard-shortcuts";
import { RIGHT_PANEL_EMPTY, useUIStore } from "@/stores/ui-store";
import { useAppStore } from "@/stores/app-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import {
  activatePane,
  activateTab,
  activateWorkspace,
  undockBrowserFromRightPanel,
} from "@/tauri/commands";
import { useSidebarDensityStore } from "@/stores/sidebar-density-store";
import { usePaneZoomStore } from "@/stores/pane-zoom-store";
import {
  setJumpTargets,
} from "@/components/layout/sidebar-inbox-jump";

// A fake KeyboardEvent — dispatch only uses the second arg when it needs to
// call preventDefault via the caller, not inside dispatch itself, so a stub
// is fine for closeOverlay-path testing.
const FAKE_EVENT = new KeyboardEvent("keydown", { key: "Escape" });

beforeEach(() => {
  vi.clearAllMocks();
  useAppStore.setState({ appState: null, pendingActiveWorkspaceId: null });
  useUIStore.setState({
    onboardingProjectDir: null,
    hasSeenOnboarding: false,
    showSettings: false,
    showFileSearch: false,
    showContentSearch: false,
    showCommandPalette: false,
    showNewWorkspaceDialog: false,
    renameWorkspaceId: null,
    themeStudio: null,
  });
  // Restore the store's boot state (GUI on) so a case that opts out
  // doesn't leak into the next one.
  const initialFlags = useFeatureFlags.getInitialState();
  useFeatureFlags.setState({
    enableAgentChat: initialFlags.enableAgentChat,
    enableLazyWorkspaceCreation: initialFlags.enableLazyWorkspaceCreation,
  });
  useChatDraftStore.setState({
    draftsById: {},
    activeHomeDraftId: null,
    projectDraftIdByPath: {},
    activeDraftId: null,
  });
  window.localStorage.clear();
});

describe("use-keyboard-shortcuts dispatch — workspace cycling", () => {
  function seedWorkspaces(activeWorkspaceId = "ws-a") {
    useAppStore.setState({
      appState: {
        active_workspace_id: activeWorkspaceId,
        workspaces: ["ws-a", "ws-b", "ws-c"].map((workspace_id) => ({
          workspace_id,
          surfaces: [],
        })),
      } as unknown as NonNullable<ReturnType<typeof useAppStore.getState>["appState"]>,
      pendingActiveWorkspaceId: null,
    });
  }

  it("routes next and previous through renderer activation in snapshot order", () => {
    seedWorkspaces("ws-b");

    expect(dispatch("nextWorkspace", FAKE_EVENT)).toBe(true);
    expect(activateWorkspace).toHaveBeenLastCalledWith("ws-c");

    useAppStore.setState({ pendingActiveWorkspaceId: null });
    expect(dispatch("prevWorkspace", FAKE_EVENT)).toBe(true);
    expect(activateWorkspace).toHaveBeenLastCalledWith("ws-a");
  });

  it("wraps around the first and last workspace", () => {
    seedWorkspaces("ws-c");
    dispatch("nextWorkspace", FAKE_EVENT);
    expect(activateWorkspace).toHaveBeenLastCalledWith("ws-a");

    seedWorkspaces("ws-a");
    dispatch("prevWorkspace", FAKE_EVENT);
    expect(activateWorkspace).toHaveBeenLastCalledWith("ws-c");
  });

  it("advances rapid presses from the optimistic pending selection", () => {
    seedWorkspaces("ws-a");

    dispatch("nextWorkspace", FAKE_EVENT);
    dispatch("nextWorkspace", FAKE_EVENT);

    expect(activateWorkspace).toHaveBeenNthCalledWith(1, "ws-b");
    expect(activateWorkspace).toHaveBeenNthCalledWith(2, "ws-c");
    expect(useAppStore.getState().pendingActiveWorkspaceId).toBe("ws-c");
  });

  it("consumes the action without invoking when no target exists", () => {
    expect(dispatch("nextWorkspace", FAKE_EVENT)).toBe(true);
    expect(activateWorkspace).not.toHaveBeenCalled();

    seedWorkspaces("ws-missing");
    expect(dispatch("prevWorkspace", FAKE_EVENT)).toBe(true);
    expect(activateWorkspace).not.toHaveBeenCalled();
  });

  it("skips activation entirely when the cycle resolves to the current workspace", () => {
    // Single workspace: next/prev wrap back onto the active id. Running the
    // activation anyway would clear an open draft composer (and fire an IPC)
    // for a switch that never happens.
    useAppStore.setState({
      appState: {
        active_workspace_id: "ws-only",
        workspaces: [{ workspace_id: "ws-only", surfaces: [] }],
      } as unknown as NonNullable<ReturnType<typeof useAppStore.getState>["appState"]>,
      pendingActiveWorkspaceId: null,
    });
    const draftStore = useChatDraftStore.getState();
    const draft = draftStore.getOrCreateHomeDraft();
    draftStore.setActiveDraft(draft.draftId);

    expect(dispatch("nextWorkspace", FAKE_EVENT)).toBe(true);
    expect(dispatch("prevWorkspace", FAKE_EVENT)).toBe(true);

    expect(activateWorkspace).not.toHaveBeenCalled();
    expect(useAppStore.getState().pendingActiveWorkspaceId).toBeNull();
    // The open draft view is untouched — no optimistic clear ever ran.
    expect(useChatDraftStore.getState().activeDraftId).toBe(draft.draftId);
  });
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
    it("closes Theme Studio before the settings surface underneath it", () => {
      useUIStore.setState({
        themeStudio: { mode: "generate" },
        showSettings: true,
      });

      const handled = dispatch("closeOverlay", FAKE_EVENT);

      expect(handled).toBe(true);
      const s = useUIStore.getState();
      expect(s.themeStudio).toBeNull();
      expect(s.showSettings).toBe(true);
    });

    it("closes the rename dialog before settings", () => {
      // The rename dialog opts out of Radix's own Escape dismissal so this
      // ladder is the single closer: one press must take exactly one layer.
      useUIStore.setState({ renameWorkspaceId: "ws-1", showSettings: true });

      const handled = dispatch("closeOverlay", FAKE_EVENT);

      expect(handled).toBe(true);
      const s = useUIStore.getState();
      expect(s.renameWorkspaceId).toBeNull();
      expect(s.showSettings).toBe(true);
    });

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
      // back to the dialog (same as the sidebar New agent button). The flags
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

    it("renameWorkspace targets the active workspace", () => {
      useAppStore.setState({
        appState: {
          active_workspace_id: "ws-2",
          workspaces: [
            { workspace_id: "ws-1", surfaces: [] },
            { workspace_id: "ws-2", surfaces: [] },
          ],
        } as unknown as NonNullable<
          ReturnType<typeof useAppStore.getState>["appState"]
        >,
      });

      const handled = dispatch("renameWorkspace", FAKE_EVENT);

      expect(handled).toBe(true);
      expect(useUIStore.getState().renameWorkspaceId).toBe("ws-2");
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

    it("leaves the combo unhandled when the slot is empty", () => {
      // Not swallowed: a jump that resolves nowhere shouldn't eat the key.
      setJumpTargets(["ws-a"]);
      const handled = dispatch("workspaceJump5", FAKE_EVENT);
      expect(handled).toBe(false);
      expect(activateWorkspace).not.toHaveBeenCalled();
    });
  });
});

type AppState = NonNullable<ReturnType<typeof useAppStore.getState>["appState"]>;

describe("use-keyboard-shortcuts dispatch — jump to needs-you", () => {
  function seed(activeId: string, blocked: string[]) {
    const ids = ["ws-a", "ws-b", "ws-c"];
    useAppStore.setState({
      appState: {
        active_workspace_id: activeId,
        workspaces: ids.map((workspace_id) => ({
          workspace_id,
          surfaces: [
            {
              surface_id: `s-${workspace_id}`,
              root: { kind: "terminal", pane_id: `p-${workspace_id}` },
            },
          ],
        })),
        pane_statuses: Object.fromEntries(
          ids.map((id) => [`p-${id}`, blocked.includes(id) ? "permission" : "idle"]),
        ),
      } as unknown as AppState,
      pendingActiveWorkspaceId: null,
    });
  }

  beforeEach(() => {
    useSidebarDensityStore.setState({ statusSince: {} });
  });

  it("goes to the longest-waiting blocked workspace, then the next one", () => {
    seed("ws-a", ["ws-b", "ws-c"]);
    useSidebarDensityStore.setState({
      statusSince: {
        "ws-b": { status: "permission", at: 200 },
        "ws-c": { status: "permission", at: 100 },
      },
    });
    expect(dispatch("jumpToNeedsYou", FAKE_EVENT)).toBe(true);
    expect(activateWorkspace).toHaveBeenLastCalledWith("ws-c");
    expect(dispatch("jumpToNeedsYou", FAKE_EVENT)).toBe(true);
    expect(activateWorkspace).toHaveBeenLastCalledWith("ws-b");
  });

  it("leaves the key alone when nothing needs the user", () => {
    seed("ws-a", []);
    expect(dispatch("jumpToNeedsYou", FAKE_EVENT)).toBe(false);
    expect(activateWorkspace).not.toHaveBeenCalled();
  });
});

describe("use-keyboard-shortcuts dispatch — tabs and panes", () => {
  const split = {
    kind: "split",
    pane_id: "split-1",
    direction: "horizontal",
    child_sizes: [0.5, 0.5],
    children: [
      { kind: "terminal", pane_id: "p-left" },
      { kind: "terminal", pane_id: "p-right" },
    ],
  };

  function seed(opts: { activeTab?: string; activePane?: string; root?: unknown } = {}) {
    useAppStore.setState({
      appState: {
        active_workspace_id: "ws-1",
        workspaces: [
          {
            workspace_id: "ws-1",
            tabs: [{ tab_id: "t-1" }, { tab_id: "t-2" }, { tab_id: "t-3" }],
            active_tab_id: opts.activeTab ?? "t-1",
            active_surface_id: "s-1",
            surfaces: [
              {
                surface_id: "s-1",
                active_pane_id: opts.activePane ?? "p-left",
                root: opts.root ?? split,
              },
            ],
          },
        ],
        pane_statuses: {},
      } as unknown as AppState,
      pendingActiveWorkspaceId: null,
    });
  }

  beforeEach(() => {
    usePaneZoomStore.setState({ zoomedPaneBySurface: {} });
    document.body.innerHTML = "";
  });

  it("cycles tabs forward and back, wrapping at either end", () => {
    seed({ activeTab: "t-3" });
    expect(dispatch("nextTab", FAKE_EVENT)).toBe(true);
    expect(activateTab).toHaveBeenLastCalledWith("ws-1", "t-1");
    seed({ activeTab: "t-1" });
    expect(dispatch("prevTab", FAKE_EVENT)).toBe(true);
    expect(activateTab).toHaveBeenLastCalledWith("ws-1", "t-3");
  });

  it("toggles a zoom on the active pane of a split", () => {
    seed({ activePane: "p-right" });
    dispatch("togglePaneZoom", FAKE_EVENT);
    expect(usePaneZoomStore.getState().zoomedPaneBySurface).toEqual({ "s-1": "p-right" });
    dispatch("togglePaneZoom", FAKE_EVENT);
    expect(usePaneZoomStore.getState().zoomedPaneBySurface).toEqual({});
  });

  it("does not zoom a lone pane", () => {
    seed({ activePane: "p-only", root: { kind: "terminal", pane_id: "p-only" } });
    expect(dispatch("togglePaneZoom", FAKE_EVENT)).toBe(true);
    expect(usePaneZoomStore.getState().zoomedPaneBySurface).toEqual({});
  });

  it("focuses the pane on screen in the pressed direction", () => {
    const place = (id: string, left: number, right: number) => {
      const el = document.createElement("div");
      el.dataset.paneDropId = id;
      el.getBoundingClientRect = () =>
        ({ left, right, top: 0, bottom: 100, width: right - left, height: 100 }) as DOMRect;
      document.body.appendChild(el);
    };
    place("p-left", 0, 100);
    place("p-right", 101, 200);
    seed({ activePane: "p-left" });

    expect(dispatch("focusPaneRight", FAKE_EVENT)).toBe(true);
    expect(activatePane).toHaveBeenLastCalledWith("p-right");

    vi.mocked(activatePane).mockClear();
    expect(dispatch("focusPaneLeft", FAKE_EVENT)).toBe(true);
    expect(activatePane).not.toHaveBeenCalled();
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
