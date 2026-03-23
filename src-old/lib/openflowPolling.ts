import type { CommLogEntry, OpenFlowRunRecord } from '../stores/types';

// Orchestration is now backend-driven; these constants are for comm log display polling only.
export const ACTIVE_COMM_LOG_INTERVAL_MS = 3000;
export const COMPLETED_COMM_LOG_INTERVAL_MS = 10000;
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
