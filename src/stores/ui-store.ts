import { create } from "zustand";

export type RightPanelTab = "changes" | "files" | "pr";

interface UIStore {
  rightPanelTabs: Record<string, RightPanelTab | null>;
  rightPanelWidth: number;
  showNewWorkspaceDialog: boolean;

  getRightPanelTab: (workspaceId: string) => RightPanelTab | null;
  setRightPanelTab: (workspaceId: string, tab: RightPanelTab | null) => void;
  toggleRightPanel: (workspaceId: string, tab: RightPanelTab) => void;
  setRightPanelWidth: (width: number) => void;
  setShowNewWorkspaceDialog: (show: boolean) => void;
}

export const useUIStore = create<UIStore>((set, get) => ({
  rightPanelTabs: {},
  rightPanelWidth: 320,
  showNewWorkspaceDialog: false,

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
}));
