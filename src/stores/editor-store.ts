import { create } from "zustand";
import { persist } from "zustand/middleware";

export interface EditorTabState {
  filePath: string | null;
  baselineContent: string;
  isDirty: boolean;
}

const DEFAULT_TAB: EditorTabState = {
  filePath: null,
  baselineContent: "",
  isDirty: false,
};

interface EditorStore {
  tabs: Record<string, EditorTabState>;
  getTab: (tabId: string) => EditorTabState | undefined;
  initTab: (tabId: string, opts?: { filePath?: string }) => void;
  setFilePath: (tabId: string, filePath: string) => void;
  setBaselineContent: (tabId: string, content: string) => void;
  setDirty: (tabId: string, dirty: boolean) => void;
  removeTab: (tabId: string) => void;
}

export const useEditorStore = create<EditorStore>()(
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
              ...(opts?.filePath != null ? { filePath: opts.filePath } : {}),
            },
          },
        })),

      setFilePath: (tabId, filePath) =>
        set((s) => ({
          tabs: {
            ...s.tabs,
            [tabId]: { ...(s.tabs[tabId] ?? DEFAULT_TAB), filePath },
          },
        })),

      setBaselineContent: (tabId, content) =>
        set((s) => ({
          tabs: {
            ...s.tabs,
            [tabId]: {
              ...(s.tabs[tabId] ?? DEFAULT_TAB),
              baselineContent: content,
              isDirty: false,
            },
          },
        })),

      setDirty: (tabId, dirty) =>
        set((s) => ({
          tabs: {
            ...s.tabs,
            [tabId]: { ...(s.tabs[tabId] ?? DEFAULT_TAB), isDirty: dirty },
          },
        })),

      removeTab: (tabId) =>
        set((s) => {
          const { [tabId]: _, ...rest } = s.tabs;
          return { tabs: rest };
        }),
    }),
    {
      name: "codemux-editor",
      partialize: (state) => ({
        tabs: Object.fromEntries(
          Object.entries(state.tabs).map(([id, tab]) => [
            id,
            { filePath: tab.filePath, baselineContent: "", isDirty: false },
          ]),
        ),
      }),
    },
  ),
);
