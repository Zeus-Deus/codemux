import { create } from "zustand";
import { persist } from "zustand/middleware";

export interface DiffTabState {
  filePath: string | null;
  staged: boolean;
  layout: "split" | "unified";
  focusMode: boolean;
  section: "staged" | "unstaged" | "against_base" | "all";
  fileIndex: number;
  baseBranch: string | null;
}

const DEFAULT_TAB: DiffTabState = {
  filePath: null,
  staged: false,
  layout: "unified",
  focusMode: false,
  section: "all",
  fileIndex: 0,
  baseBranch: null,
};

interface DiffStore {
  tabs: Record<string, DiffTabState>;
  getTab: (tabId: string) => DiffTabState | undefined;
  initTab: (tabId: string, opts?: { file?: string; staged?: boolean }) => void;
  setFile: (tabId: string, filePath: string, staged: boolean) => void;
  setLayout: (tabId: string, layout: "split" | "unified") => void;
  toggleFocusMode: (tabId: string) => void;
  setSection: (tabId: string, section: DiffTabState["section"]) => void;
  setFileIndex: (tabId: string, index: number) => void;
  setBaseBranch: (tabId: string, baseBranch: string | null) => void;
  removeTab: (tabId: string) => void;
}

export const useDiffStore = create<DiffStore>()(
  persist(
    (set, get) => ({
      tabs: {},

      getTab: (tabId) => get().tabs[tabId],

      initTab: (tabId, opts) =>
        set((s) => ({
          tabs: {
            ...s.tabs,
            [tabId]: {
              ...DEFAULT_TAB,
              ...s.tabs[tabId],
              ...(opts?.file != null ? { filePath: opts.file } : {}),
              ...(opts?.staged != null ? { staged: opts.staged } : {}),
            },
          },
        })),

      setFile: (tabId, filePath, staged) =>
        set((s) => ({
          tabs: {
            ...s.tabs,
            [tabId]: { ...(s.tabs[tabId] ?? DEFAULT_TAB), filePath, staged },
          },
        })),

      setLayout: (tabId, layout) =>
        set((s) => ({
          tabs: {
            ...s.tabs,
            [tabId]: { ...(s.tabs[tabId] ?? DEFAULT_TAB), layout },
          },
        })),

      toggleFocusMode: (tabId) =>
        set((s) => {
          const tab = s.tabs[tabId] ?? DEFAULT_TAB;
          return {
            tabs: {
              ...s.tabs,
              [tabId]: { ...tab, focusMode: !tab.focusMode },
            },
          };
        }),

      setSection: (tabId, section) =>
        set((s) => ({
          tabs: {
            ...s.tabs,
            [tabId]: { ...(s.tabs[tabId] ?? DEFAULT_TAB), section },
          },
        })),

      setFileIndex: (tabId, index) =>
        set((s) => ({
          tabs: {
            ...s.tabs,
            [tabId]: { ...(s.tabs[tabId] ?? DEFAULT_TAB), fileIndex: index },
          },
        })),

      setBaseBranch: (tabId, baseBranch) =>
        set((s) => ({
          tabs: {
            ...s.tabs,
            [tabId]: { ...(s.tabs[tabId] ?? DEFAULT_TAB), baseBranch },
          },
        })),

      removeTab: (tabId) =>
        set((s) => {
          const { [tabId]: _, ...rest } = s.tabs;
          return { tabs: rest };
        }),
    }),
    {
      name: "codemux-diff",
      partialize: (state) => ({ tabs: state.tabs }),
    },
  ),
);
