import { useAgentChatStore } from "@/stores/agent-chat-store";
import { useCustomAcpEvents } from "./use-custom-acp-events";
import { useEffect } from "react";
import { customAcpUnavailable, selectedAcpCatalog, useCustomAcp } from "@/stores/custom-acp-store";

/** Restoring identity is a read-only operation; no process starts here. */
export function useCustomAcpThread(threadId: string | null | undefined, active: boolean) {
  const warning = useCustomAcpEvents(active);
  const state = useCustomAcp();
  useEffect(() => {
    if (!active || !threadId) return;
    void useCustomAcp.getState().readThread(threadId);
  }, [active, threadId]);
  const read = threadId ? state.reads[threadId] : undefined;
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
  const effectiveWarning = [warning, changedLiveSession ? unavailable : null].filter(Boolean).join("\n") || null;
  const error = read?.error ?? (threadId ? state.errors[threadId] : null) ?? (changedLiveSession ? null : unavailable) ?? (active && read?.ready && (!agent || !agent.enabled) ? "Choose an enabled custom agent in the model picker. Configure missing agents in Settings → Agent." : null);
  return { warning: effectiveWarning, catalog, binding, agent, error, loading: active && !read?.ready && !read?.error, ready: !active || (!!threadId && !!read?.ready && !error), busy: threadId ? state.busy[threadId] ?? false : false };
}
