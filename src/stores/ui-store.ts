import { create } from "zustand";
import { persist } from "zustand/middleware";
import type { ModelSelection, PendingWorkspace } from "@/tauri/types";

export type RightPanelTab = "changes" | "files" | "review";

interface UIStore {
  rightPanelTabs: Record<string, RightPanelTab | null>;
  rightPanelWidth: number;
  showNewWorkspaceDialog: boolean;
  newWorkspaceProjectDir: string | null;
  showSettings: boolean;
  settingsSection: string | null;
  showAutomations: boolean;
  showFileSearch: boolean;
  showContentSearch: boolean;
  pendingWorkspaces: PendingWorkspace[];
  lastSelectedAgentId: string | null;
  /** Last model + reasoning choice per agent family (`claude`, `codex`,
   *  `opencode`, `gemini`), so reopening the New Workspace dialog
   *  restores the user's pick instead of resetting to Default. */
  lastModelSelections: Record<string, ModelSelection>;
  showCommandPalette: boolean;
  showCloneDialog: boolean;
  showNewProjectScreen: boolean;
  onboardingProjectDir: string | null;
  hasSeenOnboarding: boolean;
  /** Callback ref set by AppShell after SidebarProvider mounts */
  sidebarToggleFn: (() => void) | null;

  getRightPanelTab: (workspaceId: string) => RightPanelTab | null;
  setRightPanelTab: (workspaceId: string, tab: RightPanelTab | null) => void;
  toggleRightPanel: (workspaceId: string, tab: RightPanelTab) => void;
  setRightPanelWidth: (width: number) => void;
  setShowNewWorkspaceDialog: (show: boolean, projectDir?: string | null) => void;
  setShowSettings: (show: boolean, section?: string | null) => void;
  setShowAutomations: (show: boolean) => void;
  setShowFileSearch: (show: boolean) => void;
  setShowContentSearch: (show: boolean) => void;
  addPendingWorkspace: (pw: PendingWorkspace) => void;
  removePendingWorkspace: (id: string) => void;
  failPendingWorkspace: (id: string, error: string) => void;
  setLastSelectedAgentId: (id: string | null) => void;
  setLastModelSelection: (family: string, selection: ModelSelection) => void;
  setShowCommandPalette: (show: boolean) => void;
  toggleCommandPalette: () => void;
  setShowCloneDialog: (show: boolean) => void;
  setShowNewProjectScreen: (show: boolean) => void;
  setOnboardingProjectDir: (dir: string | null) => void;
  setSidebarToggleFn: (fn: (() => void) | null) => void;
}

export const useUIStore = create<UIStore>()(
  persist(
    (set, get) => ({
      rightPanelTabs: {},
      rightPanelWidth: 320,
      showNewWorkspaceDialog: false,
      newWorkspaceProjectDir: null,
      showSettings: false,
      settingsSection: null,
      showAutomations: false,
      showFileSearch: false,
      showContentSearch: false,
      pendingWorkspaces: [],
      lastSelectedAgentId: null,
      lastModelSelections: {},
      showCommandPalette: false,
      showCloneDialog: false,
      showNewProjectScreen: false,
      onboardingProjectDir: null,
      hasSeenOnboarding: false,
      sidebarToggleFn: null,

      getRightPanelTab: (workspaceId) => get().rightPanelTabs[workspaceId] ?? null,

      setRightPanelTab: (workspaceId, tab) =>
        set((s) => ({
          rightPanelTabs: { ...s.rightPanelTabs, [workspaceId]: tab },
        })),

      toggleRightPanel: (workspaceId, tab) =>
        set((s) => {
          const current = s.rightPanelTabs[workspaceId] ?? null;
          return {
            rightPanelTabs: {
              ...s.rightPanelTabs,
              [workspaceId]: current === tab ? null : tab,
            },
          };
        }),

      setRightPanelWidth: (width) =>
        set({ rightPanelWidth: Math.max(240, Math.min(500, width)) }),

      setShowNewWorkspaceDialog: (show, projectDir = null) =>
        set({ showNewWorkspaceDialog: show, newWorkspaceProjectDir: show ? (projectDir ?? null) : null }),

      setShowSettings: (show, section = null) => set({ showSettings: show, settingsSection: show ? (section ?? null) : null }),
      setShowAutomations: (show) => set({ showAutomations: show }),
      setShowFileSearch: (show) => set({ showFileSearch: show }),
      setShowContentSearch: (show) => set({ showContentSearch: show }),

      addPendingWorkspace: (pw) =>
        set((s) => ({ pendingWorkspaces: [...s.pendingWorkspaces, pw] })),

      removePendingWorkspace: (id) =>
        set((s) => ({
          pendingWorkspaces: s.pendingWorkspaces.filter((pw) => pw.id !== id),
        })),

      failPendingWorkspace: (id, error) =>
        set((s) => ({
          pendingWorkspaces: s.pendingWorkspaces.map((pw) =>
            pw.id === id ? { ...pw, status: "failed" as const, errorMessage: error } : pw,
          ),
        })),

      setLastSelectedAgentId: (id) => set({ lastSelectedAgentId: id }),

      setLastModelSelection: (family, selection) =>
        set((s) => ({
          lastModelSelections: { ...s.lastModelSelections, [family]: selection },
        })),

      setShowCommandPalette: (show) => set({ showCommandPalette: show }),
      toggleCommandPalette: () => set((s) => ({ showCommandPalette: !s.showCommandPalette })),

      setShowCloneDialog: (show) => set({ showCloneDialog: show }),

      setShowNewProjectScreen: (show) => set({ showNewProjectScreen: show }),

      setOnboardingProjectDir: (dir) =>
        set((s) =>
          dir === null
            ? { onboardingProjectDir: null, hasSeenOnboarding: true }
            : { onboardingProjectDir: dir, hasSeenOnboarding: s.hasSeenOnboarding },
        ),

      setSidebarToggleFn: (fn) => set({ sidebarToggleFn: fn }),
    }),
    {
      name: "codemux-ui",
      version: 1,
      partialize: (state) => ({
        rightPanelTabs: state.rightPanelTabs,
        rightPanelWidth: state.rightPanelWidth,
        lastSelectedAgentId: state.lastSelectedAgentId,
        lastModelSelections: state.lastModelSelections,
        hasSeenOnboarding: state.hasSeenOnboarding,
      }),
      // v0 → v1: the right-panel tab id `"pr"` was renamed to `"review"`
      // when the panel itself was renamed (Phase 3). Rewrite any persisted
      // values so users keep their active tab on upgrade instead of having
      // it silently fall back to the default.
      migrate: (persistedState, version) => {
        if (version >= 1) return persistedState;
        const state = persistedState as { rightPanelTabs?: Record<string, string | null> };
        if (state?.rightPanelTabs) {
          const migrated: Record<string, string | null> = {};
          for (const [wsId, tab] of Object.entries(state.rightPanelTabs)) {
            migrated[wsId] = tab === "pr" ? "review" : tab;
          }
          state.rightPanelTabs = migrated;
        }
        return state;
      },
    },
  ),
);
