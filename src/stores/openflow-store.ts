import { create } from "zustand";
import type {
  AgentSessionState,
  CommLogEntry,
  OpenFlowRunRecord,
  OpenFlowRuntimeSnapshot,
  OrchestratorTriggerResult,
} from "@/tauri/types";
import {
  getOpenflowRuntimeSnapshot,
  getCommunicationLog,
  getAgentSessionsForRun,
} from "@/tauri/commands";

interface OpenFlowStore {
  // State
  runtimeSnapshot: OpenFlowRuntimeSnapshot | null;
  activeRunId: string | null;
  commLog: Record<string, CommLogEntry[]>;
  commLogOffsets: Record<string, number>;
  agentSessions: Record<string, AgentSessionState[]>;
  lastCycleResult: OrchestratorTriggerResult | null;
  newRunDialogOpen: boolean;

  // Actions
  syncRuntime: () => Promise<void>;
  setActiveRun: (runId: string | null) => void;
  fetchCommLog: (runId: string) => Promise<void>;
  fetchAgentSessions: (runId: string) => Promise<void>;
  clearRunData: (runId: string) => void;
  setLastCycleResult: (result: OrchestratorTriggerResult) => void;
  setNewRunDialogOpen: (open: boolean) => void;
}

const MAX_COMM_LOG_ENTRIES = 500;

export const useOpenFlowStore = create<OpenFlowStore>((set, get) => ({
  runtimeSnapshot: null,
  activeRunId: null,
  commLog: {},
  commLogOffsets: {},
  agentSessions: {},
  lastCycleResult: null,
  newRunDialogOpen: false,

  syncRuntime: async () => {
    try {
      const snapshot = await getOpenflowRuntimeSnapshot();
      const current = get().runtimeSnapshot;
      if (current && JSON.stringify(current) === JSON.stringify(snapshot)) return;
      set({ runtimeSnapshot: snapshot });
    } catch (err) {
      console.error("Failed to sync OpenFlow runtime:", err);
    }
  },

  setActiveRun: (runId) => set({ activeRunId: runId }),

  fetchCommLog: async (runId) => {
    try {
      const offset = get().commLogOffsets[runId] ?? 0;
      const [entries, newOffset] = await getCommunicationLog(runId, offset);
      if (entries.length === 0 && newOffset === offset) return;

      set((s) => {
        const existing = s.commLog[runId] ?? [];
        const combined = [...existing, ...entries];
        const trimmed =
          combined.length > MAX_COMM_LOG_ENTRIES
            ? combined.slice(-MAX_COMM_LOG_ENTRIES)
            : combined;
        return {
          commLog: { ...s.commLog, [runId]: trimmed },
          commLogOffsets: { ...s.commLogOffsets, [runId]: newOffset },
        };
      });
    } catch (err) {
      console.error("Failed to fetch comm log:", err);
    }
  },

  fetchAgentSessions: async (runId) => {
    try {
      const sessions = await getAgentSessionsForRun(runId);
      set((s) => ({
        agentSessions: { ...s.agentSessions, [runId]: sessions },
      }));
    } catch (err) {
      console.error("Failed to fetch agent sessions:", err);
    }
  },

  clearRunData: (runId) => {
    set((s) => {
      const { [runId]: _cl, ...commLog } = s.commLog;
      const { [runId]: _co, ...commLogOffsets } = s.commLogOffsets;
      const { [runId]: _as, ...agentSessions } = s.agentSessions;
      return { commLog, commLogOffsets, agentSessions };
    });
  },

  setLastCycleResult: (result) => set({ lastCycleResult: result }),

  setNewRunDialogOpen: (open) => set({ newRunDialogOpen: open }),
}));

// Derived selectors

export function useActiveRun(): OpenFlowRunRecord | null {
  return useOpenFlowStore((s) => {
    if (!s.runtimeSnapshot || !s.activeRunId) return null;
    return (
      s.runtimeSnapshot.active_runs.find((r) => r.run_id === s.activeRunId) ??
      null
    );
  });
}

export function useActiveCommLog(): CommLogEntry[] {
  return useOpenFlowStore((s) => {
    if (!s.activeRunId) return [];
    return s.commLog[s.activeRunId] ?? [];
  });
}

export function useActiveAgentSessions(): AgentSessionState[] {
  return useOpenFlowStore((s) => {
    if (!s.activeRunId) return [];
    return s.agentSessions[s.activeRunId] ?? [];
  });
}
