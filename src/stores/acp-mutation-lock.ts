import { create } from "zustand";

/**
 * Custom ACP agent writes in flight, by agent id.
 *
 * A save replaces the agent's whole definition and the backend does not
 * reject stale revisions, so two writes started from different controls
 * (the Providers list switch, the detail panel's switch, Save, Remove)
 * can silently undo each other. Every write goes through `runAcpMutation`,
 * which holds the agent's lock until the write and its readback settle,
 * whatever panel is mounted, and every control that writes is disabled
 * while its agent is locked.
 */
interface AcpMutationLock {
  pending: Record<string, true>;
}

export const useAcpMutationLock = create<AcpMutationLock>(() => ({ pending: {} }));

export function isAcpMutating(agentId: string): boolean {
  return !!useAcpMutationLock.getState().pending[agentId];
}

export function useAcpMutating(agentId: string | null | undefined): boolean {
  return useAcpMutationLock((s) => (agentId ? !!s.pending[agentId] : false));
}

/** Run one write for `agentId`. Refuses (resolves to `undefined` without
 *  calling `work`) when another write for the same agent is in flight. */
export async function runAcpMutation<T>(agentId: string, work: () => Promise<T>): Promise<T | undefined> {
  if (isAcpMutating(agentId)) return undefined;
  useAcpMutationLock.setState((s) => ({ pending: { ...s.pending, [agentId]: true } }));
  try {
    return await work();
  } finally {
    useAcpMutationLock.setState((s) => {
      const { [agentId]: _released, ...pending } = s.pending;
      return { pending };
    });
  }
}
