import { describe, it, expect, beforeEach } from "vitest";
import { useUIStore } from "./ui-store";

const STORAGE_KEY = "codemux-ui";

beforeEach(() => {
  // Reset volatile state between tests. Zustand's persist middleware writes
  // to localStorage synchronously on every setState in jsdom, so clearing
  // localStorage AFTER the reset prevents the reset itself from leaking a
  // non-default snapshot across tests.
  useUIStore.setState({
    rightPanelTabs: {},
    rightPanelWidth: 320,
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
