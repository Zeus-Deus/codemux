import { create } from "zustand";
import { persist } from "zustand/middleware";

interface PendingSave {
  /** Live buffer while the captured version is being saved. */
  draft: string;
  done: Promise<void>;
}

interface EditorSave {
  previous?: Promise<void>;
  ownsTab: () => boolean;
  finish: (written?: string) => void;
}

export interface EditorTabState {
  filePath: string | null;
  baselineContent: string;
  isDirty: boolean;
  /** Unsaved buffer of an unmounted editor. Kept in memory only. */
  suspendedDraft?: string;
  pendingSave?: PendingSave;
  /** Transient request from a source reference. Omitted from persistence.
   *  `endColumn` (exclusive) selects the range from `column`, e.g. a search hit. */
  revealRequest?: { line: number; column?: number; endColumn?: number; nonce: number };
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
  suspendDraft: (tabId: string, filePath: string, content: string) => void;
  beginSave: (tabId: string, draft: string) => EditorSave;
  requestReveal: (tabId: string, line: number, column?: number, endColumn?: number) => void;
  clearReveal: (tabId: string, nonce: number) => void;
  removeTab: (tabId: string) => void;
}

export const useEditorStore = create<EditorStore>()(
  persist(
    (set, get) => ({
      tabs: {},

      getTab: (tabId) => get().tabs[tabId],

      initTab: (tabId, opts) =>
        set((s) => {
          const current = s.tabs[tabId] ?? DEFAULT_TAB;
          const filePath = opts?.filePath ?? current.filePath;
          const tab = filePath === current.filePath
            ? current
            : { ...DEFAULT_TAB, filePath, revealRequest: current.revealRequest };
          return { tabs: { ...s.tabs, [tabId]: tab } };
        }),

      setFilePath: (tabId, filePath) =>
        get().initTab(tabId, { filePath }),

      setBaselineContent: (tabId, content) =>
        set((s) => ({
          tabs: {
            ...s.tabs,
            [tabId]: {
              ...(s.tabs[tabId] ?? DEFAULT_TAB),
              baselineContent: content,
              isDirty: false,
              suspendedDraft: undefined,
            },
          },
        })),

      setDirty: (tabId, dirty) => {
        // Persist writes even when an updater returns the unchanged state.
        if (get().tabs[tabId]?.isDirty === dirty) return;
        set((s) => ({
          tabs: {
            ...s.tabs,
            [tabId]: { ...(s.tabs[tabId] ?? DEFAULT_TAB), isDirty: dirty },
          },
        }));
      },

      suspendDraft: (tabId, filePath, content) => {
        const tab = get().tabs[tabId];
        if (!tab?.isDirty || tab.filePath !== filePath) return;
        set((s) => ({ tabs: { ...s.tabs, [tabId]: { ...tab, suspendedDraft: content } } }));
      },

      beginSave: (tabId, draft) => {
        const tab = get().tabs[tabId];
        const previous = tab.pendingSave?.done;
        let resolve!: () => void;
        const done = new Promise<void>((release) => { resolve = release; });
        // All saves for this open file share its current buffer. Each write
        // waits for its predecessor before checking the disk version.
        const pending = tab.pendingSave ?? { draft, done };
        pending.done = done;
        if (!tab.pendingSave) {
          set((s) => ({ tabs: { ...s.tabs, [tabId]: { ...tab, pendingSave: pending } } }));
        }
        const ownsTab = () => get().tabs[tabId]?.pendingSave === pending;
        return {
          previous,
          ownsTab,
          finish(written) {
            const tab = get().tabs[tabId];
            // Closing the tab or changing files detaches the old save.
            if (tab?.pendingSave === pending) {
              const baselineContent = written ?? tab.baselineContent;
              const isDirty = pending.draft !== baselineContent;
              const updated = {
                ...tab,
                baselineContent,
                isDirty,
                suspendedDraft: isDirty ? pending.draft : undefined,
                pendingSave: pending.done === done ? undefined : pending,
              };
              set((s) => ({ tabs: { ...s.tabs, [tabId]: updated } }));
            }
            resolve();
          },
        };
      },

      requestReveal: (tabId, line, column, endColumn) =>
        set((s) => {
          const current = s.tabs[tabId] ?? DEFAULT_TAB;
          const start = column != null ? Math.max(1, Math.floor(column)) : undefined;
          const end = endColumn != null ? Math.floor(endColumn) : undefined;
          return {
            tabs: {
              ...s.tabs,
              [tabId]: {
                ...current,
                revealRequest: {
                  line: Math.max(1, Math.floor(line)),
                  ...(start != null ? { column: start } : {}),
                  ...(start != null && end != null && end > start
                    ? { endColumn: end }
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
