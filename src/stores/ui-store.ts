import { create } from "zustand";
import { persist } from "zustand/middleware";

export type RightPanelTab = "changes" | "files" | "pr";

interface UIStore {
  rightPanelTabs: Record<string, RightPanelTab | null>;
  rightPanelWidth: number;
  showNewWorkspaceDialog: boolean;
  showSettings: boolean;
  settingsSection: string | null;
  showFileSearch: boolean;
  showContentSearch: boolean;

  getRightPanelTab: (workspaceId: string) => RightPanelTab | null;
  setRightPanelTab: (workspaceId: string, tab: RightPanelTab | null) => void;
  toggleRightPanel: (workspaceId: string, tab: RightPanelTab) => void;
  setRightPanelWidth: (width: number) => void;
  setShowNewWorkspaceDialog: (show: boolean) => void;
  setShowSettings: (show: boolean, section?: string | null) => void;
  setShowFileSearch: (show: boolean) => void;
  setShowContentSearch: (show: boolean) => void;
}

export const useUIStore = create<UIStore>()(
  persist(
    (set, get) => ({
      rightPanelTabs: {},
      rightPanelWidth: 320,
      showNewWorkspaceDialog: false,
      showSettings: false,
      settingsSection: null,
      showFileSearch: false,
      showContentSearch: false,

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

      setShowNewWorkspaceDialog: (show) => set({ showNewWorkspaceDialog: show }),

      setShowSettings: (show, section = null) => set({ showSettings: show, settingsSection: show ? (section ?? null) : null }),
      setShowFileSearch: (show) => set({ showFileSearch: show }),
      setShowContentSearch: (show) => set({ showContentSearch: show }),
    }),
    {
      name: "codemux-ui",
      partialize: (state) => ({
        rightPanelTabs: state.rightPanelTabs,
        rightPanelWidth: state.rightPanelWidth,
      }),
    },
  ),
);
