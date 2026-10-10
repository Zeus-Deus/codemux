import { create } from "zustand";
import { persist } from "zustand/middleware";
import { acpAgents, acpBinding, acpThreadCatalog, acpProbe, acpSetConfig, type AcpAgent, type AcpBinding, type AcpCatalog } from "@/tauri/custom-acp";

export const acpCatalogKey = (id: string, revision: string) => JSON.stringify([id, revision]);
export function customAcpUnavailable(binding: AcpBinding | undefined, agents: AcpAgent[] | null): string | null {
  if (!binding || !agents) return null;
  const agent = agents.find(a => a.id === binding.agent_id);
  if (!agent) return "This custom agent was removed. Its history is retained; create a new chat to use another harness.";
  if (!agent.enabled) return "This custom agent is disabled. Enable it in Agent settings to resume this chat.";
  if (agent.revision !== binding.revision) return "This agent's launch configuration changed. Current sessions keep their configuration; this chat cannot be restarted with a different harness. Create a new chat.";
  return null;
}
export interface CustomAcpStore {
  selections: Record<string, string>;
  bindings: Record<string, AcpBinding>;
  agents: AcpAgent[] | null;
  catalogs: Record<string, AcpCatalog>;
  threadCatalogs: Record<string, AcpCatalog>;
  errors: Record<string, string>;
  restored: Record<string, boolean>;
  busy: Record<string, boolean>;
  /** Native readbacks/starts/events confirmed in this renderer; never persisted. */
  live: Record<string, boolean>;
  /** A delayed start must not supersede lifecycle evidence since admission. */
  markLive: (threadId: string, live: boolean, expectedEpoch?: number) => void;
  loadAgents: (force?: boolean) => Promise<AcpAgent[]>;
  select: (threadId: string, agentId: string) => void;
  restore: (threadId: string) => Promise<AcpBinding | null>;
  probe: (agentId: string, cwd?: string | null) => Promise<AcpCatalog>;
  refreshThread: (threadId: string) => Promise<AcpCatalog>;
  setConfig: (threadId: string, configId: string, value: string | boolean) => Promise<AcpCatalog>;
}
let listGeneration = 0;
let listing: Promise<AcpAgent[]> | null = null;
const restoreFlights = new Map<string, Promise<AcpBinding | null>>();
const catalogGenerations = new Map<string, number>();
const lifecycleEpochs = new Map<string, number>();
export const acpLifecycleEpoch = (threadId: string) => lifecycleEpochs.get(threadId) ?? 0;
export const useCustomAcp = create<CustomAcpStore>()(persist((set, get) => ({
  selections: {}, bindings: {}, agents: null, catalogs: {}, threadCatalogs: {}, errors: {}, restored: {}, busy: {}, live: {},
  markLive(threadId, live, expectedEpoch) {
    if (expectedEpoch !== undefined && acpLifecycleEpoch(threadId) !== expectedEpoch) return;
    lifecycleEpochs.set(threadId, acpLifecycleEpoch(threadId) + 1);
    // Lifecycle evidence owns the live bit, not a replacement catalog.
    set(s => ({ live: { ...s.live, [threadId]: live } }));
  },
  loadAgents(force = false) {
    if (listing && !force) return listing;
    const generation = ++listGeneration;
    const request = acpAgents().then(values => {
      // Defensive redaction even if an older backend returns values.
      const agents = values.map(a => ({ ...a, environment: Object.fromEntries(Object.keys(a.environment).map(k => [k, null])) }));
      if (generation === listGeneration) set({ agents });
      return agents;
    }).finally(() => { if (listing === request) listing = null; });
    listing = request;
    return request;
  },
  select(threadId, agentId) {
    if (get().bindings[threadId] || get().busy[threadId]) return;
    set(s => ({ selections: { ...s.selections, [threadId]: agentId } }));
  },
  restore(threadId) {
    const existing = restoreFlights.get(threadId);
    if (existing) return existing;
    const previousCatalog = get().threadCatalogs[threadId];
    const request = acpBinding(threadId).then(binding => {
      set(s => {
        const bindings = { ...s.bindings };
        const threadCatalogs = { ...s.threadCatalogs };
        if (binding) {
          const current = s.bindings[threadId];
          // A post-start metadata read is not newer catalog authority than
          // an exact-owner catalog accepted while that metadata was pending.
          const newerCatalog = threadCatalogs[threadId] && threadCatalogs[threadId] !== previousCatalog &&
            binding.thread_id === threadId && current?.thread_id === threadId &&
            current.agent_id === binding.agent_id && current.revision === binding.revision &&
            current.cwd === binding.cwd && current.session_id === binding.session_id;
          bindings[threadId] = binding;
          if (!newerCatalog) threadCatalogs[threadId] = binding.catalog;
        }
        else { delete bindings[threadId]; delete threadCatalogs[threadId]; }
        const errors = { ...s.errors }; delete errors[threadId];
        return { bindings, threadCatalogs, errors, restored: { ...s.restored, [threadId]: true }, selections: binding ? { ...s.selections, [threadId]: binding.agent_id } : s.selections };
      });
      return binding;
    }).catch(error => {
      set(s => ({ restored: { ...s.restored, [threadId]: false }, errors: { ...s.errors, [threadId]: String(error) } }));
      throw error;
    }).finally(() => { if (restoreFlights.get(threadId) === request) restoreFlights.delete(threadId); });
    restoreFlights.set(threadId, request);
    return request;
  },
  async probe(agentId, cwd = null) {
    const agent = get().agents?.find(a => a.id === agentId);
    if (!agent) throw new Error("Save the custom agent before probing it.");
    const key = acpCatalogKey(agent.id, agent.revision);
    const generation = (catalogGenerations.get(key) ?? 0) + 1;
    catalogGenerations.set(key, generation);
    set(s => ({ busy: { ...s.busy, [key]: true } }));
    try {
      const catalog = await acpProbe(agentId, cwd);
      if (get().agents?.find(a => a.id === agentId)?.revision === agent.revision && catalogGenerations.get(key) === generation) {
        set(s => ({ catalogs: { ...s.catalogs, [key]: catalog } }));
      }
      return catalog;
    } finally { if (catalogGenerations.get(key) === generation) set(s => ({ busy: { ...s.busy, [key]: false } })); }
  },
  async refreshThread(threadId) {
    const binding = get().bindings[threadId];
    if (!binding || binding.thread_id !== threadId) throw new Error("Custom agent binding identity mismatch.");
    const generation = (catalogGenerations.get(threadId) ?? 0) + 1;
    catalogGenerations.set(threadId, generation);
    const lifecycleEpoch = acpLifecycleEpoch(threadId) + 1;
    // Admission fences delayed start confirmation, even before this read settles.
    lifecycleEpochs.set(threadId, lifecycleEpoch);
    const { catalog, live } = await acpThreadCatalog(threadId);
    if (typeof live !== "boolean") throw new Error("Custom agent native status unavailable.");
    const current = get().bindings[threadId];
    if (!current || current.thread_id !== threadId || current.agent_id !== binding.agent_id ||
        current.revision !== binding.revision || current.cwd !== binding.cwd ||
        current.session_id !== binding.session_id || catalog.agent_id !== binding.agent_id) {
      throw new Error("Custom agent catalog identity mismatch.");
    }
    if (catalogGenerations.get(threadId) !== generation) return get().threadCatalogs[threadId] ?? catalog;
    set(s => ({ threadCatalogs: { ...s.threadCatalogs, [threadId]: catalog },
      live: acpLifecycleEpoch(threadId) === lifecycleEpoch ? { ...s.live, [threadId]: live } : s.live }));
    return catalog;
  },
  async setConfig(threadId, configId, value) {
    if (get().busy[threadId]) throw new Error("Wait for the current configuration change.");
    set(s => ({ busy: { ...s.busy, [threadId]: true } }));
    catalogGenerations.set(threadId, (catalogGenerations.get(threadId) ?? 0) + 1);
    try {
      await acpSetConfig(threadId, configId, value);
      // Read back after acceptance: a native idle update can supersede the
      // setter's response before it reaches this renderer.
      return await get().refreshThread(threadId);
    } finally { set(s => ({ busy: { ...s.busy, [threadId]: false } })); }
  },
}), { name: "codemux:custom-acp:v1", partialize: s => ({ selections: s.selections }) }));
export function selectedAcpCatalog(state: CustomAcpStore, threadId: string | null | undefined): AcpCatalog | null {
  if (!threadId) return null;
  if (state.bindings[threadId]) return state.threadCatalogs[threadId] ?? state.bindings[threadId].catalog;
  const agent = state.agents?.find(a => a.id === state.selections[threadId]);
  return agent ? state.catalogs[acpCatalogKey(agent.id, agent.revision)] ?? null : null;
}
