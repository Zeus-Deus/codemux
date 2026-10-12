import { create } from "zustand";
import { persist } from "zustand/middleware";
import type { WorkflowLimits, WorkflowRoute } from "@/tauri/workflows";

export interface WorkflowDraft {
  title: string;
  goal: string;
  source: string;
  allowWrites: boolean;
  routes: WorkflowRoute[];
  limits: WorkflowLimits;
}

interface WorkflowUIStore {
  drafts: Record<string, WorkflowDraft>;
  selectedRuns: Record<string, string | null>;
  setDraft: (workspaceId: string, draft: WorkflowDraft) => void;
  selectRun: (workspaceId: string, runId: string | null) => void;
}

/** Editor drafts stay local; authoritative runs and scripts belong to the host. */
export const useWorkflowUIStore = create<WorkflowUIStore>()(
  persist(
    (set) => ({
      drafts: {},
      selectedRuns: {},
      setDraft: (workspaceId, draft) => set((state) => ({
        drafts: { ...state.drafts, [workspaceId]: draft },
      })),
      selectRun: (workspaceId, runId) => set((state) => ({
        selectedRuns: { ...state.selectedRuns, [workspaceId]: runId },
      })),
    }),
    { name: "codemux:workflow-ui:v1" },
  ),
);
