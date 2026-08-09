import { create } from "zustand";
import { persist } from "zustand/middleware";

export interface EditorTabState {
  filePath: string | null;
  baselineContent: string;
  isDirty: boolean;
  /** Transient request from a source reference. Omitted from persistence. */
  revealRequest?: { line: number; column?: number; nonce: number };
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
  requestReveal: (tabId: string, line: number, column?: number) => void;
  clearReveal: (tabId: string, nonce: number) => void;
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

      requestReveal: (tabId, line, column) =>
        set((s) => {
          const current = s.tabs[tabId] ?? DEFAULT_TAB;
          return {
            tabs: {
              ...s.tabs,
              [tabId]: {
                ...current,
                revealRequest: {
                  line: Math.max(1, Math.floor(line)),
                  ...(column != null
                    ? { column: Math.max(1, Math.floor(column)) }
                    : {}),
                  nonce: (current.revealRequest?.nonce ?? 0) + 1,
                },
              },
            },
          };
        }),

      /**
       * Consume the request a pane just applied. Without this the request
       * sticks around and replays on every remount of the pane — the right
       * panel mounts only the active pane, so every tab switch would reset
       * the cursor and re-centre the scroll. The nonce is matched so a newer
       * citation that landed between the reveal and this call survives.
       */
      clearReveal: (tabId, nonce) =>
        set((s) => {
          const current = s.tabs[tabId];
          if (!current?.revealRequest || current.revealRequest.nonce !== nonce) {
            return s;
          }
          const { revealRequest: _consumed, ...rest } = current;
          void _consumed;
          return { tabs: { ...s.tabs, [tabId]: rest } };
        }),

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
