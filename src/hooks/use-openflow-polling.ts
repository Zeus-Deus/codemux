import { useEffect, useRef } from "react";
import { useTauriEvent } from "./use-tauri-event";
import { onOpenflowCycle } from "@/tauri/events";
import { useOpenFlowStore } from "@/stores/openflow-store";
import type { OrchestratorTriggerResult } from "@/tauri/types";
import {
  commLogPollInterval,
  POLL_RUNTIME_FALLBACK,
} from "@/lib/openflow-utils";
import { useActiveRun } from "@/stores/openflow-store";

export function useOpenFlowPolling(runId: string | null) {
  const syncRuntime = useOpenFlowStore((s) => s.syncRuntime);
  const fetchCommLog = useOpenFlowStore((s) => s.fetchCommLog);
  const fetchAgentSessions = useOpenFlowStore((s) => s.fetchAgentSessions);
  const setLastCycleResult = useOpenFlowStore((s) => s.setLastCycleResult);
  const activeRun = useActiveRun();

  // Subscribe to backend cycle events
  useTauriEvent<OrchestratorTriggerResult>(
    onOpenflowCycle,
    (result) => {
      setLastCycleResult(result);
      syncRuntime();
      if (runId) fetchCommLog(runId);
    },
    [runId],
  );

  // Comm log polling interval
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null);

  useEffect(() => {
    if (!runId) return;

    // Initial fetch
    fetchCommLog(runId);
    fetchAgentSessions(runId);

    const ms = commLogPollInterval(activeRun);
    intervalRef.current = setInterval(() => {
      fetchCommLog(runId);
    }, ms);

    return () => {
      if (intervalRef.current) clearInterval(intervalRef.current);
    };
  }, [runId, activeRun?.status]);

  // Agent sessions: re-fetch when run status changes
  useEffect(() => {
    if (runId) fetchAgentSessions(runId);
  }, [runId, activeRun?.status]);

  // Runtime snapshot fallback polling
  useEffect(() => {
    syncRuntime();
    const id = setInterval(syncRuntime, POLL_RUNTIME_FALLBACK);
    return () => clearInterval(id);
  }, []);
}
