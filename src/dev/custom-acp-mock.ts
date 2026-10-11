import type { AcpAgent, AcpAgentInput, AcpBinding, AcpCatalog } from "@/tauri/custom-acp";
/** Synthetic browser fixtures, with the same redaction/routing boundaries as IPC. */
export function createCustomAcpMock(emit: (event: string, payload: unknown) => void) {
  let sequence = 0;
  const agents = new Map<string, AcpAgent>();
  const bindings = new Map<string, AcpBinding>();
  const live = new Set<string>();
  const redact = (agent: AcpAgent): AcpAgent => structuredClone({ ...agent, environment: Object.fromEntries(Object.keys(agent.environment).map(key => [key, null])) });
  const catalogFor = (agent: AcpAgent): AcpCatalog => ({ agent_id: agent.id, agent_name: agent.name, current_model: null, supports_resume: true, supports_images: false, auth_methods: [], capabilities: { models: [], effort_granularity: "per_session", effort_label_map: {}, permission_modes: [], default_permission_mode: null, permission_granularity: "per_session", supports_steering: false }, config_options: [{ id: "reasoning /fixture", name: "Reasoning", description: "Synthetic runtime control", category: "thought_level", type: "select", current_value: "balanced", options: [{ value: "balanced", name: "Balanced", description: null }, { value: " Exact:Deep ", name: "Deep", description: null }] }, { id: "thinking", name: "Thinking", description: null, category: null, type: "boolean", current_value: false, options: [] }] });
  const assertAvailable = (binding: AcpBinding) => {
    const agent = agents.get(binding.agent_id);
    if (!agent || !agent.enabled || agent.revision !== binding.revision) throw new Error("Custom agent unavailable: removed, disabled or launch configuration changed.");
    return agent;
  };
  const readThread = (threadId: string) => {
    const binding = bindings.get(threadId);
    if (!binding) throw new Error("Custom agent session not found.");
    if (!live.has(threadId)) assertAvailable(binding);
    return { catalog: structuredClone(binding.catalog), live: live.has(threadId) };
  };
  const handlers: Record<string, (args: Record<string, unknown>) => unknown> = {
    acp_agents: () => Array.from(agents.values(), redact),
    acp_save_agent: ({ input }) => {
      const value = input as AcpAgentInput;
      if (!value.name.trim() || !value.executable.trim()) throw new Error("Agent name and executable are required.");
      const previous = value.id ? agents.get(value.id) : undefined;
      if (value.id && !previous) throw new Error("Custom agent not found.");
      const environment: Record<string, string> = {};
      for (const [key, val] of Object.entries(value.environment)) {
        if (val !== null) environment[key] = val;
        else if (previous?.environment[key] != null) environment[key] = previous.environment[key];
        else throw new Error("Cannot retain a missing environment variable.");
      }
      const launchChanged = !previous || JSON.stringify([previous.executable, previous.args, previous.environment, previous.enabled, previous.auth_method]) !== JSON.stringify([value.executable, value.args, environment, value.enabled, value.auth_method]);
      const agent: AcpAgent = { ...value, environment, id: previous?.id ?? `mock-acp-${++sequence}`, revision: launchChanged ? `mock-revision-${++sequence}` : previous!.revision };
      agents.set(agent.id, agent); emit("custom_acp_changed", null); return redact(agent);
    },
    acp_delete_agent: ({ agentId }) => { agents.delete(String(agentId)); emit("custom_acp_changed", null); },
    acp_probe: ({ agentId }) => { const agent = agents.get(String(agentId)); if (!agent?.enabled) throw new Error("Custom agent unavailable."); return catalogFor(agent); },
    acp_binding: ({ threadId }) => structuredClone(bindings.get(String(threadId)) ?? null),
    acp_catalog: ({ threadId }) => readThread(String(threadId)).catalog,
    acp_thread_catalog: ({ threadId }) => readThread(String(threadId)),
    agent_chat_stop_session: ({ provider, threadId }) => { if (provider === "acp") live.delete(String(threadId)); },
    acp_set_config: ({ threadId, configId, value }) => {
      const binding = bindings.get(String(threadId)); if (!binding) throw new Error("Custom agent session not found."); assertAvailable(binding);
      const option = binding.catalog.config_options.find(option => option.id === configId);
      if (!option || (option.type === "boolean" ? typeof value !== "boolean" : typeof value !== "string" || !option.options.some(o => o.value === value))) throw new Error("Unsupported configuration value.");
      option.current_value = value as string | boolean;
      binding.config_values[option.id] = value as string | boolean;
      emit("custom_acp_catalog_changed", { thread_id: binding.thread_id, catalog: structuredClone(binding.catalog) });
      return structuredClone(binding.catalog);
    },
  };
  return { handlers,
    start(input: { thread_id: string; cwd: string; model: string | null; extra?: unknown }) {
      const existing = bindings.get(input.thread_id);
      const id = (input.extra as { acp_agent_id?: string } | undefined)?.acp_agent_id;
      if (existing) { if (id && id !== existing.agent_id) throw new Error("Thread bound to a different custom agent."); assertAvailable(existing); live.add(input.thread_id); return; }
      const agent = id ? agents.get(id) : undefined;
      if (!agent?.enabled) throw new Error("Choose an enabled custom agent.");
      const catalog = catalogFor(agent);
      if (input.model !== null && !catalog.capabilities.models.some(m => m.id === input.model)) throw new Error("Model not advertised.");
      bindings.set(input.thread_id, { thread_id: input.thread_id, agent_id: agent.id, revision: agent.revision, cwd: input.cwd, session_id: `mock-native-${input.thread_id}`, catalog, config_values: {} });
      live.add(input.thread_id);
    },
    setModel(threadId: string, model: string) {
      const binding = bindings.get(threadId); if (!binding) throw new Error("Custom agent session not found."); assertAvailable(binding);
      if (!binding.catalog.capabilities.models.some(m => m.id === model)) throw new Error("Model not advertised.");
      binding.catalog.current_model = model;
      emit("custom_acp_catalog_changed", { thread_id: threadId, catalog: structuredClone(binding.catalog) });
    },
  };
}
