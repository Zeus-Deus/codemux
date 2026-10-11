import { useEffect, useState } from "react";
import { onCustomAcpCatalogChanged, onCustomAcpChanged, type UnlistenFn } from "@/tauri/events";
import { useCustomAcp } from "@/stores/custom-acp-store";
const observers = new Set<(error: string) => void>();
let generation = 0;
let cleanups: UnlistenFn[] = [];
/** One native subscription shared by settings and all mounted chat pickers. */
export function useCustomAcpEvents(active = true) {
  const [warning, setWarning] = useState<string | null>(null);
  useEffect(() => {
    if (!active) return;
    const observer = (error: string) => setWarning(error);
    observers.add(observer);
    if (observers.size === 1) {
      const epoch = ++generation;
      const report = (e: unknown) => { if (generation === epoch) for (const cb of observers) cb(`Custom agent updates unavailable: ${String(e)}`); };
      const own = (promise: Promise<UnlistenFn>) => { void promise.then(unlisten => { if (epoch === generation && observers.size) cleanups.push(unlisten); else unlisten(); }).catch(report); };
      own(onCustomAcpChanged(() => {
        if (generation === epoch) void useCustomAcp.getState().loadAgents(true).catch(report);
      }));
      own(onCustomAcpCatalogChanged(({ thread_id }) => {
        if (generation !== epoch || !useCustomAcp.getState().bindings[thread_id]) return;
        // Events invalidate; the native command remains the read authority.
        void useCustomAcp.getState().refreshThread(thread_id).catch(report);
      }));
    }
    return () => {
      observers.delete(observer);
      if (!observers.size) { generation++; for (const cleanup of cleanups) cleanup(); cleanups = []; }
    };
  }, [active]);
  return warning;
}
