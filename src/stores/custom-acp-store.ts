import { create } from "zustand";
import { persist } from "zustand/middleware";
import { agentChatSetModel } from "@/tauri/commands";
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
  /** Shared initial/readback status; a binding cache is not a valid readback. */
  reads: Record<string, { ready: boolean; error: string | null }>;
  busy: Record<string, boolean>;
  /** Native readbacks/starts/events confirmed in this renderer; never persisted. */
  live: Record<string, boolean>;
  /** A delayed start must not supersede lifecycle evidence since admission. */
  markLive: (threadId: string, live: boolean, expectedEpoch?: number) => void;
  loadAgents: (force?: boolean) => Promise<AcpAgent[]>;
  select: (threadId: string, agentId: string) => void;
  restore: (threadId: string) => Promise<AcpBinding | null>;
  readThread: (threadId: string) => Promise<void>;
  probe: (agentId: string, cwd?: string | null) => Promise<AcpCatalog>;
  refreshThread: (threadId: string) => Promise<AcpCatalog>;
  setConfig: (threadId: string, configId: string, value: string | boolean) => Promise<AcpCatalog>;
  setModel: (threadId: string, model: string) => Promise<AcpCatalog>;
}
let listGeneration = 0;
let listing: Promise<AcpAgent[]> | null = null;
/** Retain the latest outcome for obsolete initial metadata consumers. */
let latestListing: { promise: Promise<AcpAgent[]>; generation: number } | null = null;
const restoreFlights = new Map<string, Promise<AcpBinding | null>>();
interface ThreadReadFlight {
  promise: Promise<void>;
  generation: number;
  /** One meaningful repair accepted while this initial catalog was pending. */
  repair?: { agentId: string; revision: string };
}
const readFlights = new Map<string, ThreadReadFlight>();
const readGenerations = new Map<string, number>();
const catalogGenerations = new Map<string, number>();
const catalogFlights = new Map<string, { promise: Promise<AcpCatalog>; generation: number }>();
const lifecycleEpochs = new Map<string, number>();
export const acpLifecycleEpoch = (threadId: string) => lifecycleEpochs.get(threadId) ?? 0;
export const useCustomAcp = create<CustomAcpStore>()(persist((set, get) => ({
  selections: {}, bindings: {}, agents: null, catalogs: {}, threadCatalogs: {}, errors: {}, restored: {}, reads: {}, busy: {}, live: {},
  markLive(threadId, live, expectedEpoch) {
    if (expectedEpoch !== undefined && acpLifecycleEpoch(threadId) !== expectedEpoch) return;
    lifecycleEpochs.set(threadId, acpLifecycleEpoch(threadId) + 1);
    // Lifecycle evidence owns the live bit, not a replacement catalog.
    set(s => ({ live: { ...s.live, [threadId]: live } }));
  },
  loadAgents(force = false) {
    if (listing && !force) return listing;
    const generation = ++listGeneration;
    const latest = (): Promise<AcpAgent[]> => {
      if (latestListing && latestListing.generation > generation && latestListing.generation === listGeneration) return latestListing.promise;
      throw new Error("Custom agent listing superseded before authoritative readback.");
    };
    const request = acpAgents().then(values => {
      if (generation !== listGeneration) return latest();
      // Defensive redaction even if an older backend returns values.
      const agents = values.map(a => ({ ...a, environment: Object.fromEntries(Object.keys(a.environment).map(k => [k, null])) }));
      if (generation === listGeneration) {
        const previous = get().agents;
        set({ agents });
        // A definition change is one explicit retry trigger, not a polling
        // dependency on store snapshots. Keep failures until valid readback.
        if (force) for (const [threadId, read] of Object.entries(get().reads)) {
          const id = get().bindings[threadId]?.agent_id ?? get().selections[threadId];
          const before = previous?.find(a => a.id === id);
          const after = agents.find(a => a.id === id);
          if (after?.enabled && !get().busy[threadId] &&
              (before?.enabled !== after.enabled || before?.revision !== after.revision)) {
            const flight = readFlights.get(threadId);
            if (!read.ready && get().bindings[threadId] && flight && flight.generation === readGenerations.get(threadId)) {
              // Do not replace the pending initial read. If it fails, consume
              // this exact definition repair once before shared completion.
              flight.repair = { agentId: after.id, revision: after.revision };
            } else if (read.error) {
              void (get().bindings[threadId] ? get().refreshThread(threadId) : get().readThread(threadId)).catch(() => {});
            }
          }
        }
      }
      // Completion, like publication, belongs to the latest listing. Keep
      // settled success/failure distinct from a merely populated agent cache.
      return generation !== listGeneration ? latest() : agents;
    }, error => {
      if (generation !== listGeneration) return latest();
      throw error;
    }).finally(() => { if (listing === request) listing = null; });
    listing = request;
    latestListing = { promise: request, generation };
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
    const readGeneration = readGenerations.get(threadId);
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
      if (readGenerations.get(threadId) === readGeneration) set(s => ({ restored: { ...s.restored, [threadId]: false }, errors: { ...s.errors, [threadId]: String(error) } }));
      throw error;
    }).finally(() => { if (restoreFlights.get(threadId) === request) restoreFlights.delete(threadId); });
    restoreFlights.set(threadId, request);
    return request;
  },
  readThread(threadId) {
    const existing = readFlights.get(threadId);
    if (existing && existing.generation === readGenerations.get(threadId) && !get().reads[threadId]?.ready) return existing.promise;
    const generation = (readGenerations.get(threadId) ?? 0) + 1;
    readGenerations.set(threadId, generation);
    set(s => ({ reads: { ...s.reads, [threadId]: { ready: false, error: s.reads[threadId]?.error ?? null } } }));
    let flight!: ThreadReadFlight;
    const request = (async () => {
      try {
        await Promise.all([get().loadAgents(), get().restore(threadId)]);
        if (readGenerations.get(threadId) !== generation) return;
        if (get().bindings[threadId]) {
          const refreshing = get().refreshThread(threadId);
          flight.generation = readGenerations.get(threadId)!;
          await refreshing;
        }
        else set(s => ({ reads: { ...s.reads, [threadId]: { ready: true, error: null } } }));
      } catch (error) {
        if (readGenerations.get(threadId) === generation) set(s => ({ reads: { ...s.reads, [threadId]: { ready: false, error: String(error) } } }));
        const repair = flight.repair;
        delete flight.repair;
        const current = get().bindings[threadId];
        const agent = get().agents?.find(a => a.id === repair?.agentId);
        if (repair && current?.agent_id === repair.agentId && agent?.enabled && agent.revision === repair.revision &&
            readFlights.get(threadId) === flight && readGenerations.get(threadId) === flight.generation &&
            get().reads[threadId]?.error && !get().busy[threadId]) {
          // Share the single trailing retry with remounted/duplicate consumers,
          // but never retry its rejection without another meaningful repair.
          const retry = get().refreshThread(threadId);
          flight.generation = readGenerations.get(threadId)!;
          await retry.catch(() => {});
        }
      }
    })().finally(() => { if (readFlights.get(threadId) === flight) readFlights.delete(threadId); });
    flight = { promise: request, generation };
    readFlights.set(threadId, flight);
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
    const previousReadGeneration = readGenerations.get(threadId);
    const readGeneration = (previousReadGeneration ?? 0) + 1;
    readGenerations.set(threadId, readGeneration);
    const initial = readFlights.get(threadId);
    // The pending shared initial completion follows replacement catalog
    // authority, including a repair accepted before either read settles.
    // A mutation or a new metadata read retires that completion instead.
    if (initial && initial.generation === previousReadGeneration && !get().reads[threadId]?.ready && !get().busy[threadId]) {
      initial.generation = readGeneration;
    }
    const lifecycleEpoch = acpLifecycleEpoch(threadId) + 1;
    // Admission fences delayed start confirmation, even before this read settles.
    lifecycleEpochs.set(threadId, lifecycleEpoch);
    const fail = (error: unknown): Promise<AcpCatalog> => {
      const before = catalogGenerations.get(threadId);
      if (readGenerations.get(threadId) === readGeneration) set(s => ({ reads: { ...s.reads, [threadId]: { ready: false, error: String(error) } } }));
      // Error publication can also synchronously admit replacement authority.
      if (catalogGenerations.get(threadId) !== before) return latest();
      throw error;
    };
    const latest = (): Promise<AcpCatalog> => {
      const flight = catalogFlights.get(threadId);
      // Only a strictly newer authority can be joined. Keep its settled
      // promise too: cached metadata cannot prove a newer read succeeded.
      if (flight && flight.generation > generation && flight.generation === catalogGenerations.get(threadId)) return flight.promise;
      return fail(new Error("Custom agent catalog read superseded before authoritative readback."));
    };
    const request = acpThreadCatalog(threadId).then(({ catalog, live }) => {
      if (catalogGenerations.get(threadId) !== generation) return latest();
      if (typeof live !== "boolean") return fail(new Error("Custom agent native status unavailable."));
      const current = get().bindings[threadId];
      if (!current || current.thread_id !== threadId || current.agent_id !== binding.agent_id ||
          current.revision !== binding.revision || current.cwd !== binding.cwd ||
          current.session_id !== binding.session_id || catalog.agent_id !== binding.agent_id) {
        return fail(new Error("Custom agent catalog identity mismatch."));
      }
      set(s => {
        const errors = { ...s.errors }; delete errors[threadId];
        return { threadCatalogs: { ...s.threadCatalogs, [threadId]: catalog }, errors,
          reads: { ...s.reads, [threadId]: { ready: true, error: null } },
          live: acpLifecycleEpoch(threadId) === lifecycleEpoch ? { ...s.live, [threadId]: live } : s.live };
      });
      // A subscriber may invalidate synchronously while this state publishes.
      return catalogGenerations.get(threadId) !== generation ? latest() : catalog;
    }, error => {
      // Handoff is outside an enclosing catch: a newer rejection must not
      // be mistaken for this flight's error or cause a self-await.
      return catalogGenerations.get(threadId) !== generation ? latest() : fail(error);
    });
    catalogFlights.set(threadId, { promise: request, generation });
    return request;
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
  async setModel(threadId, model) {
    if (get().busy[threadId]) throw new Error("Wait for the current configuration change.");
    // Acquire before the first await. Model and config setters share one
    // per-thread owner, including authoritative post-ACK catalog readback.
    set(s => ({ busy: { ...s.busy, [threadId]: true } }));
    catalogGenerations.set(threadId, (catalogGenerations.get(threadId) ?? 0) + 1);
    try {
      await agentChatSetModel("acp", threadId, model);
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
