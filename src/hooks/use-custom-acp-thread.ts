import { useAgentChatStore } from "@/stores/agent-chat-store";
import { useCustomAcpEvents } from "./use-custom-acp-events";
import { useEffect, useState } from "react";
import { customAcpUnavailable, selectedAcpCatalog, useCustomAcp } from "@/stores/custom-acp-store";

/** Restoring identity is a read-only operation; no process starts here. */
export function useCustomAcpThread(threadId: string | null | undefined, active: boolean) {
  const warning = useCustomAcpEvents(active);
  const state = useCustomAcp();
  const [readError, setReadError] = useState<string | null>(null);
  const [loadedThread, setLoadedThread] = useState<string | null>(null);
  useEffect(() => {
    if (!active || !threadId) return;
    let disposed = false;
    setReadError(null);
    void (async () => {
      try {
        await Promise.all([useCustomAcp.getState().loadAgents(), useCustomAcp.getState().restore(threadId)]);
        if (disposed) return;
        if (useCustomAcp.getState().bindings[threadId]) await useCustomAcp.getState().refreshThread(threadId);
        if (!disposed) setLoadedThread(threadId);
      } catch (e) { if (!disposed) setReadError(String(e)); }
    })();
    return () => { disposed = true; };
  }, [active, threadId]);
  const binding = threadId ? state.bindings[threadId] : undefined;
  const selected = threadId ? state.selections[threadId] : undefined;
  const agent = state.agents?.find(a => a.id === selected);
  const catalog = selectedAcpCatalog(state, threadId);
  useEffect(() => {
    if (active && threadId && binding && catalog) {
      useAgentChatStore.getState().ensureThread(threadId);
      useAgentChatStore.getState().setModel(threadId, catalog.current_model);
    }
  }, [active, threadId, binding, catalog?.current_model]);
  const unavailable = customAcpUnavailable(binding, state.agents);
  const changedLiveSession = !!threadId && !!state.live[threadId] && !!binding && !!agent?.enabled && binding.revision !== agent.revision;
  const effectiveWarning = changedLiveSession ? unavailable : warning;
  const error = readError ?? (threadId ? state.errors[threadId] : null) ?? (changedLiveSession ? null : unavailable) ?? (active && loadedThread === threadId && (!agent || !agent.enabled) ? "Choose an enabled custom agent in the model picker. Configure missing agents in Settings → Agent." : null);
  return { warning: effectiveWarning, catalog, binding, agent, error, loading: active && loadedThread !== threadId && !readError, ready: !active || (!!threadId && loadedThread === threadId && !error), busy: threadId ? state.busy[threadId] ?? false : false };
}
