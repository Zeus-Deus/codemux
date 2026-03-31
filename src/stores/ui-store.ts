import { create } from "zustand";
import { persist } from "zustand/middleware";
import type { PendingWorkspace } from "@/tauri/types";

export type RightPanelTab = "changes" | "files" | "pr";

interface UIStore {
  rightPanelTabs: Record<string, RightPanelTab | null>;
  rightPanelWidth: number;
  showNewWorkspaceDialog: boolean;
  newWorkspaceProjectDir: string | null;
  showSettings: boolean;
  settingsSection: string | null;
  showFileSearch: boolean;
  showContentSearch: boolean;
  pendingWorkspaces: PendingWorkspace[];
  lastSelectedAgentId: string | null;
  showCommandPalette: boolean;
  showCloneDialog: boolean;
  showNewProjectScreen: boolean;
  onboardingProjectDir: string | null;
  /** Callback ref set by AppShell after SidebarProvider mounts */
  sidebarToggleFn: (() => void) | null;

  getRightPanelTab: (workspaceId: string) => RightPanelTab | null;
  setRightPanelTab: (workspaceId: string, tab: RightPanelTab | null) => void;
  toggleRightPanel: (workspaceId: string, tab: RightPanelTab) => void;
  setRightPanelWidth: (width: number) => void;
  setShowNewWorkspaceDialog: (show: boolean, projectDir?: string | null) => void;
  setShowSettings: (show: boolean, section?: string | null) => void;
  setShowFileSearch: (show: boolean) => void;
  setShowContentSearch: (show: boolean) => void;
  addPendingWorkspace: (pw: PendingWorkspace) => void;
  removePendingWorkspace: (id: string) => void;
  failPendingWorkspace: (id: string, error: string) => void;
  setLastSelectedAgentId: (id: string | null) => void;
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
      showFileSearch: false,
      showContentSearch: false,
      pendingWorkspaces: [],
      lastSelectedAgentId: null,
      showCommandPalette: false,
      showCloneDialog: false,
      showNewProjectScreen: false,
      onboardingProjectDir: null,
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

      setShowCommandPalette: (show) => set({ showCommandPalette: show }),
      toggleCommandPalette: () => set((s) => ({ showCommandPalette: !s.showCommandPalette })),

      setShowCloneDialog: (show) => set({ showCloneDialog: show }),

      setShowNewProjectScreen: (show) => set({ showNewProjectScreen: show }),

      setOnboardingProjectDir: (dir) => set({ onboardingProjectDir: dir }),

      setSidebarToggleFn: (fn) => set({ sidebarToggleFn: fn }),
    }),
    {
      name: "codemux-ui",
      partialize: (state) => ({
        rightPanelTabs: state.rightPanelTabs,
        rightPanelWidth: state.rightPanelWidth,
        lastSelectedAgentId: state.lastSelectedAgentId,
      }),
    },
  ),
);
