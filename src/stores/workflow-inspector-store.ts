import { create } from "zustand";

export interface WorkflowInspectorState {
  selectedTask: string | null;
  settledOpen: boolean;
  visible: Record<string, number>;
}

export const DEFAULT_WORKFLOW_INSPECTOR: WorkflowInspectorState = {
  selectedTask: null,
  settledOpen: false,
  visible: { "needs-you": 100, "ready-review": 100, "in-flight": 100, settled: 100 },
};

interface WorkflowInspectorStore {
  runs: Record<string, WorkflowInspectorState>;
  update: (runId: string, patch: Partial<WorkflowInspectorState>) => void;
}

/** Pane context survives layout remounts, without persisting execution state. */
export const useWorkflowInspectorStore = create<WorkflowInspectorStore>((set) => ({
  runs: {},
  update: (runId, patch) => set((state) => {
    const previous = state.runs[runId] ?? DEFAULT_WORKFLOW_INSPECTOR;
    if (Object.entries(patch).every(([key, value]) => previous[key as keyof WorkflowInspectorState] === value)) return state;
    const runs = { ...state.runs };
    // Refresh insertion order so recent inspectors stay available. This is
    // intentionally ephemeral and bounded even when the host retains years of runs.
    delete runs[runId];
    runs[runId] = { ...previous, ...patch };
    if (Object.keys(runs).length > 32) delete runs[Object.keys(runs)[0]];
    return { runs };
  }),
}));
