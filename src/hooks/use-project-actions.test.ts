/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import type { AppStateSnapshot, WorkspaceSnapshot } from "@/tauri/types";

// ── Mock Tauri commands ──
vi.mock("@/tauri/commands", () => ({
  pickFolderDialog: vi.fn(),
  checkIsGitRepo: vi.fn().mockResolvedValue(true),
  initGitRepo: vi.fn().mockResolvedValue(undefined),
  dbAddRecentProject: vi.fn().mockResolvedValue(undefined),
  gitCloneRepo: vi.fn(),
  createEmptyWorkspace: vi.fn().mockResolvedValue("ws-new"),
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
}));

import {
  pickFolderDialog,
  checkIsGitRepo,
  gitCloneRepo,
  createEmptyWorkspace,
  activateWorkspace,
} from "@/tauri/commands";
import { useProjectActions } from "./use-project-actions";
import { useAppStore } from "@/stores/app-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useUIStore } from "@/stores/ui-store";

const mockPickFolderDialog = vi.mocked(pickFolderDialog);
const mockCheckIsGitRepo = vi.mocked(checkIsGitRepo);
const mockGitCloneRepo = vi.mocked(gitCloneRepo);
const mockCreateEmptyWorkspace = vi.mocked(createEmptyWorkspace);
const mockActivateWorkspace = vi.mocked(activateWorkspace);

function makeWs(id: string): WorkspaceSnapshot {
  return {
    workspace_id: id,
    title: `Workspace ${id}`,
    workspace_type: "standard",
    cwd: "/some/path",
    git_branch: null,
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    worktree_path: null,
    project_root: null,
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    notification_count: 0,
    notifications_muted: false,
    latest_agent_state: null,
    tabs: [],
    active_tab_id: "",
    active_surface_id: "",
    surfaces: [],
  };
}

function makeAppState(workspaces: WorkspaceSnapshot[]): AppStateSnapshot {
  // The gate only reads `workspaces.length`. Stub the rest with empty
  // defaults so the shape satisfies AppStateSnapshot without pulling in
  // fixtures for fields the gate doesn't care about.
  return {
    schema_version: 1,
    active_workspace_id: workspaces[0]?.workspace_id ?? "",
    workspaces,
    terminal_sessions: [],
    browser_sessions: [],
    agent_browser_sessions: [],
    notifications: [],
    detected_ports: [],
    pane_statuses: {},
    persistence: {} as AppStateSnapshot["persistence"],
    config: {} as AppStateSnapshot["config"],
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  mockCheckIsGitRepo.mockResolvedValue(true);
  mockCreateEmptyWorkspace.mockResolvedValue("ws-new");
  mockActivateWorkspace.mockResolvedValue(undefined);

  // Reset both stores to a pristine state.
  useAppStore.setState({ appState: null });
  useUIStore.setState({
    onboardingProjectDir: null,
    hasSeenOnboarding: false,
  });
  // The legacy first-project onboarding wizard only fires when the
  // Agent Chat GUI is off (see `shouldUseLegacyOnboarding`). The flags
  // store now boots ON to match the backend default, so pin the CLI
  // opt-out state these cases are about.
  useFeatureFlags.setState({
    enableAgentChat: false,
    enableLazyWorkspaceCreation: false,
  });
  window.localStorage.clear();
});

describe("useProjectActions — onboarding trigger gate", () => {
  describe("openProject", () => {
    it("fires onboarding when workspaces empty AND hasSeenOnboarding false", async () => {
      mockPickFolderDialog.mockResolvedValue("/home/user/fresh-project");
      useAppStore.setState({ appState: makeAppState([]) });

      const { result } = renderHook(() => useProjectActions());
      await act(async () => {
        await result.current.openProject();
      });

      expect(useUIStore.getState().onboardingProjectDir).toBe(
        "/home/user/fresh-project",
      );
    });

    it("does NOT fire onboarding when hasSeenOnboarding true (the re-trap fix)", async () => {
      mockPickFolderDialog.mockResolvedValue("/home/user/second-project");
      useAppStore.setState({ appState: makeAppState([]) });
      useUIStore.setState({ hasSeenOnboarding: true });

      const { result } = renderHook(() => useProjectActions());
      await act(async () => {
        await result.current.openProject();
      });

      expect(useUIStore.getState().onboardingProjectDir).toBeNull();
      // Temp workspace still created — the fix only suppresses the wizard.
      expect(mockCreateEmptyWorkspace).toHaveBeenCalledWith(
        "/home/user/second-project",
      );
      expect(mockActivateWorkspace).toHaveBeenCalledWith("ws-new");
    });

    it("does NOT fire onboarding when workspaces already exist", async () => {
      mockPickFolderDialog.mockResolvedValue("/home/user/another");
      useAppStore.setState({ appState: makeAppState([makeWs("ws-existing")]) });

      const { result } = renderHook(() => useProjectActions());
      await act(async () => {
        await result.current.openProject();
      });

      expect(useUIStore.getState().onboardingProjectDir).toBeNull();
    });

    it("does NOT fire onboarding when both gates fail (workspaces exist AND seen)", async () => {
      mockPickFolderDialog.mockResolvedValue("/home/user/p");
      useAppStore.setState({ appState: makeAppState([makeWs("ws-existing")]) });
      useUIStore.setState({ hasSeenOnboarding: true });

      const { result } = renderHook(() => useProjectActions());
      await act(async () => {
        await result.current.openProject();
      });

      expect(useUIStore.getState().onboardingProjectDir).toBeNull();
    });

    it("early-returns without effects when folder picker is cancelled", async () => {
      mockPickFolderDialog.mockResolvedValue(null);
      useAppStore.setState({ appState: makeAppState([]) });

      const { result } = renderHook(() => useProjectActions());
      const res = await act(async () => result.current.openProject());

      expect(res).toEqual({ success: false });
      expect(mockCreateEmptyWorkspace).not.toHaveBeenCalled();
      expect(useUIStore.getState().onboardingProjectDir).toBeNull();
    });
  });

  describe("cloneProject", () => {
    it("fires onboarding when workspaces empty AND hasSeenOnboarding false", async () => {
      mockGitCloneRepo.mockResolvedValue("/home/user/cloned");
      useAppStore.setState({ appState: makeAppState([]) });

      const { result } = renderHook(() => useProjectActions());
      await act(async () => {
        await result.current.cloneProject("git@host:u/r.git", "/home/user");
      });

      expect(useUIStore.getState().onboardingProjectDir).toBe(
        "/home/user/cloned",
      );
    });

    it("does NOT fire onboarding when hasSeenOnboarding true (the re-trap fix)", async () => {
      mockGitCloneRepo.mockResolvedValue("/home/user/cloned-again");
      useAppStore.setState({ appState: makeAppState([]) });
      useUIStore.setState({ hasSeenOnboarding: true });

      const { result } = renderHook(() => useProjectActions());
      await act(async () => {
        await result.current.cloneProject("git@host:u/r.git", "/home/user");
      });

      expect(useUIStore.getState().onboardingProjectDir).toBeNull();
      // Temp workspace still created — the fix only suppresses the wizard.
      expect(mockCreateEmptyWorkspace).toHaveBeenCalledWith(
        "/home/user/cloned-again",
      );
    });

    it("does NOT fire onboarding when workspaces already exist", async () => {
      mockGitCloneRepo.mockResolvedValue("/home/user/cloned-x");
      useAppStore.setState({ appState: makeAppState([makeWs("ws-existing")]) });

      const { result } = renderHook(() => useProjectActions());
      await act(async () => {
        await result.current.cloneProject("git@host:u/r.git", "/home/user");
      });

      expect(useUIStore.getState().onboardingProjectDir).toBeNull();
    });
  });

  describe("integration — the full re-trap scenario", () => {
    it("second openProject after skip does not re-arm onboarding", async () => {
      // Scenario: truly first-time user opens project #1, skips onboarding,
      // closes all workspaces, opens project #2. Onboarding must not re-fire.
      mockPickFolderDialog.mockResolvedValueOnce("/home/user/p1");
      useAppStore.setState({ appState: makeAppState([]) });

      const { result } = renderHook(() => useProjectActions());

      // First open triggers onboarding.
      await act(async () => {
        await result.current.openProject();
      });
      expect(useUIStore.getState().onboardingProjectDir).toBe("/home/user/p1");

      // User clicks skip → setOnboardingProjectDir(null) → flag flips true.
      act(() => {
        useUIStore.getState().setOnboardingProjectDir(null);
      });
      expect(useUIStore.getState().hasSeenOnboarding).toBe(true);

      // User closes all workspaces (simulate empty list again).
      useAppStore.setState({ appState: makeAppState([]) });

      // Second openProject MUST NOT re-trigger onboarding.
      mockPickFolderDialog.mockResolvedValueOnce("/home/user/p2");
      await act(async () => {
        await result.current.openProject();
      });
      expect(useUIStore.getState().onboardingProjectDir).toBeNull();
    });
  });
});
