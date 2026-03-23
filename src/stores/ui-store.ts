import { create } from "zustand";

export type RightPanelTab = "changes" | "files" | "pr";

interface UIStore {
  rightPanelTabs: Record<string, RightPanelTab | null>;

  getRightPanelTab: (workspaceId: string) => RightPanelTab | null;
  setRightPanelTab: (workspaceId: string, tab: RightPanelTab | null) => void;
  toggleRightPanel: (workspaceId: string, tab: RightPanelTab) => void;
}

export const useUIStore = create<UIStore>((set, get) => ({
  rightPanelTabs: {},

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
}));
