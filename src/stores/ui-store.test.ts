import { describe, it, expect, beforeEach } from "vitest";
import { RIGHT_PANEL_MIN_WIDTH } from "@/lib/right-panel-width";
import { RIGHT_PANEL_EMPTY, useUIStore } from "./ui-store";

const STORAGE_KEY = "codemux-ui";

beforeEach(() => {
  // Reset volatile state between tests. Zustand's persist middleware writes
  // to localStorage synchronously on every setState in jsdom, so clearing
  // localStorage AFTER the reset prevents the reset itself from leaking a
  // non-default snapshot across tests.
  useUIStore.setState({
    rightPanelTabs: {},
    rightPanelWidth: 320,
    rightPanelMaximized: false,
    showNewWorkspaceDialog: false,
    newWorkspaceProjectDir: null,
    showSettings: false,
    settingsSection: null,
    showFileSearch: false,
    showContentSearch: false,
    pendingWorkspaces: [],
    lastSelectedAgentId: null,
    showCommandPalette: false,
    showCloneDialog: false,
    showNewProjectScreen: false,
    onboardingProjectDir: null,
    hasSeenOnboarding: false,
    sidebarToggleFn: null,
    expandProjectRequest: null,
  });
  window.localStorage.clear();
});

describe("ui-store — onboarding state", () => {
  it("starts with onboardingProjectDir null and hasSeenOnboarding false", () => {
    const s = useUIStore.getState();
    expect(s.onboardingProjectDir).toBeNull();
    expect(s.hasSeenOnboarding).toBe(false);
  });

  describe("setOnboardingProjectDir", () => {
    it("with a path sets the dir and leaves hasSeenOnboarding unchanged (false)", () => {
      useUIStore.getState().setOnboardingProjectDir("/home/user/myproj");

      const s = useUIStore.getState();
      expect(s.onboardingProjectDir).toBe("/home/user/myproj");
      expect(s.hasSeenOnboarding).toBe(false);
    });

    it("with a path leaves hasSeenOnboarding unchanged when already true", () => {
      // Simulate a returning user who already saw onboarding once.
      useUIStore.setState({ hasSeenOnboarding: true });
      useUIStore.getState().setOnboardingProjectDir("/home/user/another");

      const s = useUIStore.getState();
      expect(s.onboardingProjectDir).toBe("/home/user/another");
      expect(s.hasSeenOnboarding).toBe(true);
    });

    it("with null clears the dir and flips hasSeenOnboarding to true", () => {
      // Seed an active onboarding session.
      useUIStore.setState({ onboardingProjectDir: "/home/user/myproj" });
      expect(useUIStore.getState().hasSeenOnboarding).toBe(false);

      useUIStore.getState().setOnboardingProjectDir(null);

      const s = useUIStore.getState();
      expect(s.onboardingProjectDir).toBeNull();
      expect(s.hasSeenOnboarding).toBe(true);
    });

    it("with null is idempotent when hasSeenOnboarding is already true", () => {
      useUIStore.setState({
        onboardingProjectDir: "/home/user/myproj",
        hasSeenOnboarding: true,
      });

      useUIStore.getState().setOnboardingProjectDir(null);

      const s = useUIStore.getState();
      expect(s.onboardingProjectDir).toBeNull();
      expect(s.hasSeenOnboarding).toBe(true);
    });

    it("with null flips the flag even when dir was already null", () => {
      // Defensive: a caller that clears onboarding without ever setting a dir
      // (e.g. an Escape dispatch before onboarding is active) must still
      // count as "seen" so subsequent flips remain consistent.
      expect(useUIStore.getState().onboardingProjectDir).toBeNull();

      useUIStore.getState().setOnboardingProjectDir(null);

      expect(useUIStore.getState().hasSeenOnboarding).toBe(true);
    });

    it("full lifecycle: enter → exit flips hasSeenOnboarding exactly once", () => {
      // Enter
      useUIStore.getState().setOnboardingProjectDir("/p1");
      expect(useUIStore.getState().hasSeenOnboarding).toBe(false);

      // Exit via any path (skip, complete, auto-dismiss — all end up here)
      useUIStore.getState().setOnboardingProjectDir(null);
      expect(useUIStore.getState().hasSeenOnboarding).toBe(true);

      // Second open (e.g. user opens another project) must not re-flip or
      // reset the flag.
      useUIStore.getState().setOnboardingProjectDir("/p2");
      expect(useUIStore.getState().hasSeenOnboarding).toBe(true);

      useUIStore.getState().setOnboardingProjectDir(null);
      expect(useUIStore.getState().hasSeenOnboarding).toBe(true);
    });
  });

  describe("persistence", () => {
    it("persists hasSeenOnboarding to localStorage", () => {
      useUIStore.getState().setOnboardingProjectDir(null);

      const raw = window.localStorage.getItem(STORAGE_KEY);
      expect(raw).not.toBeNull();
      const persisted = JSON.parse(raw!);
      // Zustand persist format: { state: {...}, version: N }
      expect(persisted.state.hasSeenOnboarding).toBe(true);
    });

    it("does NOT persist onboardingProjectDir (session-only)", () => {
      useUIStore.getState().setOnboardingProjectDir("/home/user/myproj");

      const raw = window.localStorage.getItem(STORAGE_KEY);
      expect(raw).not.toBeNull();
      const persisted = JSON.parse(raw!);
      // onboardingProjectDir must stay transient so a crash during onboarding
      // doesn't re-arm the wizard on next launch.
      expect(persisted.state.onboardingProjectDir).toBeUndefined();
    });

    it("does NOT persist transient overlay flags", () => {
      useUIStore.setState({
        showSettings: true,
        showFileSearch: true,
        showContentSearch: true,
        showCommandPalette: true,
        showCloneDialog: true,
        showNewProjectScreen: true,
        showNewWorkspaceDialog: true,
      });
      // Force a persist write.
      useUIStore.getState().setOnboardingProjectDir(null);

      const persisted = JSON.parse(window.localStorage.getItem(STORAGE_KEY)!);
      expect(persisted.state.showSettings).toBeUndefined();
      expect(persisted.state.showFileSearch).toBeUndefined();
      expect(persisted.state.showContentSearch).toBeUndefined();
      expect(persisted.state.showCommandPalette).toBeUndefined();
      expect(persisted.state.showCloneDialog).toBeUndefined();
      expect(persisted.state.showNewProjectScreen).toBeUndefined();
      expect(persisted.state.showNewWorkspaceDialog).toBeUndefined();
    });

    it("does NOT persist the transient expand-project request", () => {
      useUIStore.getState().requestExpandProject("/home/user/proj");
      // Force a persist write.
      useUIStore.getState().setOnboardingProjectDir(null);

      const persisted = JSON.parse(window.localStorage.getItem(STORAGE_KEY)!);
      expect(persisted.state.expandProjectRequest).toBeUndefined();
    });

    it("keeps the width the user asked for, not the width that fits today", () => {
      // The layout-aware limit lives in `right-panel-width.ts` and is
      // applied at render time. Clamping on the way in would permanently
      // shrink a panel sized on a wide monitor the first time the app
      // opened on a narrow one.
      useUIStore.getState().setRightPanelWidth(1400);
      expect(useUIStore.getState().rightPanelWidth).toBe(1400);
    });

    it("still refuses a width below the panel minimum", () => {
      useUIStore.getState().setRightPanelWidth(10);
      expect(useUIStore.getState().rightPanelWidth).toBe(
        RIGHT_PANEL_MIN_WIDTH,
      );
    });

    it("does not persist the measured row width", () => {
      useUIStore.getState().setRightPanelRowWidth(1720);
      useUIStore.getState().setOnboardingProjectDir(null);

      const persisted = JSON.parse(window.localStorage.getItem(STORAGE_KEY)!);
      expect(persisted.state.rightPanelRowWidth).toBeUndefined();
    });

    it("persists hasSeenOnboarding alongside existing allowlist fields", () => {
      useUIStore.setState({
        rightPanelWidth: 400,
        lastSelectedAgentId: "builtin-claude",
      });
      useUIStore.getState().setOnboardingProjectDir(null);

      const persisted = JSON.parse(window.localStorage.getItem(STORAGE_KEY)!);
      expect(persisted.state.hasSeenOnboarding).toBe(true);
      expect(persisted.state.rightPanelWidth).toBe(400);
      expect(persisted.state.lastSelectedAgentId).toBe("builtin-claude");
    });
  });
});

describe("ui-store — expand-project request (Needs-you jump)", () => {
  it("starts with no pending expand request", () => {
    expect(useUIStore.getState().expandProjectRequest).toBeNull();
  });

  it("requestExpandProject records the target project path", () => {
    useUIStore.getState().requestExpandProject("/home/user/alpha");
    expect(useUIStore.getState().expandProjectRequest).toBe("/home/user/alpha");
  });

  it("clearExpandProjectRequest clears a matching request", () => {
    useUIStore.getState().requestExpandProject("/home/user/alpha");
    useUIStore.getState().clearExpandProjectRequest("/home/user/alpha");
    expect(useUIStore.getState().expandProjectRequest).toBeNull();
  });

  it("clearExpandProjectRequest is a no-op for a non-matching path (newer request wins)", () => {
    // A stale consumer must not clobber a request that has since moved to a
    // different group.
    useUIStore.getState().requestExpandProject("/home/user/beta");
    useUIStore.getState().clearExpandProjectRequest("/home/user/alpha");
    expect(useUIStore.getState().expandProjectRequest).toBe("/home/user/beta");
  });
});

// Full-expand is the panel's ⤢ control: the panel takes the whole content
// row and the workspace column collapses to zero width beside it. It stores
// no width of its own — that is what makes "restore" exact.
describe("ui-store — right-panel full expand", () => {
  it("is off by default and only toggles while the panel is open", () => {
    expect(useUIStore.getState().rightPanelMaximized).toBe(false);

    useUIStore.getState().toggleRightPanelMaximized("ws-1");
    expect(useUIStore.getState().rightPanelMaximized).toBe(false);

    useUIStore.getState().setRightPanelTab("ws-1", "files");
    useUIStore.getState().toggleRightPanelMaximized("ws-1");
    expect(useUIStore.getState().rightPanelMaximized).toBe(true);
  });

  it("leaves the stored width untouched, so restoring is exact", () => {
    useUIStore.getState().setRightPanelWidth(612);
    useUIStore.getState().setRightPanelTab("ws-1", "files");

    useUIStore.getState().toggleRightPanelMaximized("ws-1");
    expect(useUIStore.getState().rightPanelWidth).toBe(612);

    useUIStore.getState().toggleRightPanelMaximized("ws-1");
    expect(useUIStore.getState().rightPanelMaximized).toBe(false);
    expect(useUIStore.getState().rightPanelWidth).toBe(612);
  });

  it("clears on collapse, so the app can't come back to a hidden workspace", () => {
    useUIStore.getState().setRightPanelTab("ws-1", "files");
    useUIStore.getState().toggleRightPanelMaximized("ws-1");
    useUIStore.getState().setRightPanelTab("ws-1", null);
    expect(useUIStore.getState().rightPanelMaximized).toBe(false);
  });

  it("is never persisted", () => {
    useUIStore.getState().setRightPanelTab("ws-1", "files");
    useUIStore.getState().toggleRightPanelMaximized("ws-1");
    const persisted = JSON.parse(localStorage.getItem(STORAGE_KEY)!);
    expect(persisted.state).not.toHaveProperty("rightPanelMaximized");
  });
});

// `RIGHT_PANEL_EMPTY` is "open, showing the surface picker" — a third state
// beside "collapsed" (null) and "showing pane X".
describe("ui-store — the empty-panel sentinel", () => {
  it("opens the panel without joining the deck", () => {
    useUIStore.setState({ rightPanelPanes: { "ws-1": ["files"] } });
    useUIStore.getState().setRightPanelTab("ws-1", RIGHT_PANEL_EMPTY);

    expect(useUIStore.getState().getRightPanelTab("ws-1")).toBe(
      RIGHT_PANEL_EMPTY,
    );
    expect(useUIStore.getState().getRightPanelPanes("ws-1")).toEqual(["files"]);
  });

  it("catches the last closed pane instead of collapsing the panel", () => {
    useUIStore.setState({
      rightPanelPanes: { "ws-1": ["files"] },
      rightPanelTabs: { "ws-1": "files" },
    });
    useUIStore.getState().closeRightPanelPane("ws-1", "files");

    expect(useUIStore.getState().getRightPanelPanes("ws-1")).toEqual([]);
    expect(useUIStore.getState().getRightPanelTab("ws-1")).toBe(
      RIGHT_PANEL_EMPTY,
    );
  });
});
