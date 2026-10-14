import { create } from "zustand";
import { persist } from "zustand/middleware";

/** The custom-script editor's draft. Routes, limits and file access come
 *  from Settings → Workflows at launch, so older persisted fields are ignored. */
export interface WorkflowDraft {
  title: string;
  goal: string;
  source: string;
}

/** A run started from a chat with `/workflow`. The card renders after the
 *  last persisted transcript event that existed when it was launched. */
export interface ChatWorkflowLink {
  runId: string;
  workspaceId: string;
  anchorEventId: number | null;
  createdAt: number;
}

/** Chats keep their most recent links; older runs stay in the Workflows pane. */
const MAX_LINKS_PER_THREAD = 50;

interface WorkflowUIStore {
  drafts: Record<string, WorkflowDraft>;
  selectedRuns: Record<string, string | null>;
  /** Custom-script editor open in the Workflows pane, per workspace. */
  scriptEditor: Record<string, boolean>;
  chatRuns: Record<string, ChatWorkflowLink[]>;
  setDraft: (workspaceId: string, draft: WorkflowDraft) => void;
  selectRun: (workspaceId: string, runId: string | null) => void;
  setScriptEditor: (workspaceId: string, open: boolean) => void;
  linkChatRun: (threadId: string, link: ChatWorkflowLink) => void;
}

/** Editor drafts stay local; authoritative runs and scripts belong to the host. */
export const useWorkflowUIStore = create<WorkflowUIStore>()(
  persist(
    (set) => ({
      drafts: {},
      selectedRuns: {},
      scriptEditor: {},
      chatRuns: {},
      setDraft: (workspaceId, draft) => set((state) => ({
        drafts: { ...state.drafts, [workspaceId]: draft },
      })),
      selectRun: (workspaceId, runId) => set((state) => ({
        selectedRuns: { ...state.selectedRuns, [workspaceId]: runId },
        scriptEditor: runId ? { ...state.scriptEditor, [workspaceId]: false } : state.scriptEditor,
      })),
      setScriptEditor: (workspaceId, open) => set((state) => ({
        scriptEditor: { ...state.scriptEditor, [workspaceId]: open },
        selectedRuns: open ? { ...state.selectedRuns, [workspaceId]: null } : state.selectedRuns,
      })),
      linkChatRun: (threadId, link) => set((state) => {
        const existing = (state.chatRuns[threadId] ?? []).filter((item) => item.runId !== link.runId);
        return { chatRuns: { ...state.chatRuns, [threadId]: [...existing, link].slice(-MAX_LINKS_PER_THREAD) } };
      }),
    }),
    {
      name: "codemux:workflow-ui:v1",
      // The open editor is a momentary view, not something to restore.
      partialize: ({ drafts, selectedRuns, chatRuns }) => ({ drafts, selectedRuns, chatRuns }),
    },
  ),
);
