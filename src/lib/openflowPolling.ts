import type { CommLogEntry, OpenFlowRunRecord } from '../stores/types';

export const INITIAL_ORCHESTRATOR_DELAY_MS = 3000;
export const ORCHESTRATOR_INTERVAL_MS = 15000;
export const ACTIVE_COMM_LOG_INTERVAL_MS = 5000;
export const COMPLETED_COMM_LOG_INTERVAL_MS = 30000;
export const MAX_COMM_LOG_ENTRIES = 500;

export function commLogPollInterval(run: OpenFlowRunRecord | null) {
    return run?.status === 'completed' ? COMPLETED_COMM_LOG_INTERVAL_MS : ACTIVE_COMM_LOG_INTERVAL_MS;
}

export function mergeCommLogEntries(existing: CommLogEntry[], incoming: CommLogEntry[]) {
    if (incoming.length === 0) {
        return existing;
    }

    const combined = [...existing, ...incoming];
    if (combined.length <= MAX_COMM_LOG_ENTRIES) {
        return combined;
    }

    return combined.slice(-MAX_COMM_LOG_ENTRIES);
}
