import { beforeEach, expect, it, vi } from "vitest";
import { agentChatStartSession } from "@/tauri/commands";
import { useCustomAcp, acpCatalogKey, customAcpUnavailable } from "./custom-acp-store";
import { acpCatalog, type AcpAgent, type AcpBinding, type AcpCatalog, type AcpThreadCatalog } from "@/tauri/custom-acp";
const caps = { models: [], effort_granularity: "per_session" as const, effort_label_map: {}, permission_modes: [], default_permission_mode: null, permission_granularity: "per_session" as const };
const agent: AcpAgent = { id: "agent-a", name: "Local harness", executable: "/harness", args: ["two words", "$(literal)"], environment: { TOKEN: null }, enabled: true, auth_method: null, revision: "revision-a" };
const catalog: AcpCatalog = { agent_id: agent.id, agent_name: agent.name, capabilities: caps, config_options: [], current_model: null, supports_images: false, supports_resume: true, auth_methods: [] };
const binding: AcpBinding = { thread_id: "restored", agent_id: agent.id, revision: agent.revision, cwd: "/repo", session_id: "opaque /session", catalog, config_values: {} };
const { invoke } = vi.hoisted(() => ({ invoke: vi.fn<(command: string, payload?: unknown) => Promise<unknown>>(async command => command === "acp_binding" ? null : "thread") }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
beforeEach(() => {
  invoke.mockReset();
  invoke.mockImplementation(async (command: string) => command === "acp_binding" ? null : "thread");
  useCustomAcp.setState({ selections: {}, bindings: {}, agents: null, catalogs: {}, threadCatalogs: {}, errors: {}, restored: {}, reads: {}, busy: {}, live: {} });
});
it.each([
  { thread_id: "foreign" }, { agent_id: "other" }, { revision: "new" },
  { cwd: "/other" }, { session_id: "different native session" },
])("does not apply a delayed read across a replaced binding: %j", async replacement => {
  useCustomAcp.setState({ bindings: { restored: binding }, threadCatalogs: { restored: catalog } });
  let resolve!: (value: AcpThreadCatalog) => void;
  invoke.mockImplementation(async () => new Promise<AcpThreadCatalog>(r => { resolve = r; }));
  const reading = useCustomAcp.getState().refreshThread("restored");
  useCustomAcp.setState({ bindings: { restored: { ...binding, ...replacement } } });
  resolve({ catalog: { ...catalog, current_model: "stale" }, live: true });
  await expect(reading).rejects.toThrow(/identity mismatch/);
  expect(useCustomAcp.getState().live.restored).toBeUndefined();
  expect(useCustomAcp.getState().threadCatalogs.restored).toEqual(catalog);
});
it("fails closed when the backend omits explicit native liveness", async () => {
  useCustomAcp.setState({ bindings: { restored: binding }, threadCatalogs: { restored: catalog } });
  invoke.mockResolvedValueOnce({ catalog });
  await expect(useCustomAcp.getState().refreshThread("restored")).rejects.toThrow(/native status unavailable/);
  expect(useCustomAcp.getState().live.restored).toBeUndefined();
});
it("retains the legacy catalog-only wrapper without granting liveness", async () => {
  invoke.mockResolvedValueOnce(catalog);
  expect(await acpCatalog("restored")).toEqual(catalog);
  expect(invoke).toHaveBeenCalledExactlyOnceWith("acp_catalog", { threadId: "restored" });
  expect(useCustomAcp.getState().live).toEqual({});
});
it("lists saved instances without probing or retaining secrets", async () => {
  expect(typeof useCustomAcp.getState().loadAgents).toBe("function");
  invoke.mockResolvedValueOnce([{ ...agent, environment: { TOKEN: "must-not-retain" } }] as never);
  await useCustomAcp.getState().loadAgents();
  expect(useCustomAcp.getState().agents?.[0].environment).toEqual({ TOKEN: null });
  expect(invoke).toHaveBeenCalledExactlyOnceWith("acp_agents");
});
it.each([false, true].flatMap(oldFails => [false, true].flatMap(latestFails => [false, true].map(settled => ({ oldFails, latestFails, settled })))))
("FE3: obsolete listing follows actual latest outcome (oldFails=$oldFails latestFails=$latestFails settled=$settled)", async ({ oldFails, latestFails, settled }) => {
  const pending: { resolve: (value: AcpAgent[]) => void; reject: (reason: Error) => void }[] = [];
  invoke.mockImplementation(async command => {
    expect(command).toBe("acp_agents");
    return new Promise<AcpAgent[]>((resolve, reject) => pending.push({ resolve, reject }));
  });
  useCustomAcp.setState({ agents: [agent] });
  const oldest = useCustomAcp.getState().loadAgents();
  let oldSettled = false;
  const outcome = oldest.then(value => ({ value, error: null }), error => ({ value: null, error: String(error) })).finally(() => { oldSettled = true; });
  const newest = useCustomAcp.getState().loadAgents(true);
  const latestOutcome = newest.catch(() => null);
  const current = { ...agent, name: "Latest", environment: { TOKEN: "synthetic-only", EXTRA: null } };
  const finishLatest = () => latestFails ? pending[1].reject(new Error("latest authoritative listing failed")) : pending[1].resolve([current]);
  try {
    if (settled) { finishLatest(); await latestOutcome; }
    if (oldFails) pending[0].reject(new Error("obsolete listing failed"));
    else pending[0].resolve([{ ...agent, name: "Obsolete" }]);
    for (let tick = 0; tick < 12; tick++) await Promise.resolve();
    if (!settled) {
      expect(oldSettled).toBe(false);
      expect(useCustomAcp.getState().loadAgents()).toBe(newest);
      finishLatest();
    }
    await latestOutcome;
    expect(await outcome).toEqual(latestFails ? { value: null, error: "Error: latest authoritative listing failed" } : { value: [{ ...current, environment: { TOKEN: null, EXTRA: null } }], error: null });
    expect(useCustomAcp.getState().agents).toEqual(latestFails ? [agent] : [{ ...current, environment: { TOKEN: null, EXTRA: null } }]);
    expect(pending).toHaveLength(2);
    const fresh = useCustomAcp.getState().loadAgents();
    expect(pending).toHaveLength(3);
    pending[2].resolve([agent]);
    await expect(fresh).resolves.toEqual([agent]);
  } finally {
    for (const request of pending) request.resolve([agent]);
    await Promise.all([outcome, latestOutcome]);
  }
});
it("FE3: chained listing ownership shares only the pending latest producer", async () => {
  const pending: { resolve: (value: AcpAgent[]) => void; reject: (reason: Error) => void }[] = [];
  invoke.mockImplementation(async () => new Promise<AcpAgent[]>((resolve, reject) => pending.push({ resolve, reject })));
  const first = useCustomAcp.getState().loadAgents();
  const firstOutcome = first.catch(error => String(error));
  const second = useCustomAcp.getState().loadAgents(true);
  const secondOutcome = second.catch(error => String(error));
  pending[0].reject(new Error("first failed"));
  await Promise.resolve();
  const third = useCustomAcp.getState().loadAgents(true);
  pending[1].resolve([{ ...agent, name: "retired intermediate" }]);
  for (let tick = 0; tick < 12; tick++) await Promise.resolve();
  expect(useCustomAcp.getState().loadAgents()).toBe(third);
  const latest = { ...agent, name: "third accepted" };
  pending[2].resolve([latest]);
  await expect(firstOutcome).resolves.toEqual([latest]);
  await expect(secondOutcome).resolves.toEqual([latest]);
  await expect(third).resolves.toEqual([latest]);
  expect(pending).toHaveLength(3);
});

it("restores backend identity over a remembered selection and locks instance changes", async () => {
  useCustomAcp.getState().select("restored", "wrong");
  invoke.mockResolvedValueOnce(binding as never);
  await useCustomAcp.getState().restore("restored");
  useCustomAcp.getState().select("restored", "other");
  expect(useCustomAcp.getState().selections.restored).toBe(agent.id);
  expect(useCustomAcp.getState().threadCatalogs.restored).toEqual(catalog);
});
it("rejects missing, disabled and changed launch revisions without choosing another harness", () => {
  expect(typeof customAcpUnavailable).toBe("function");
  expect(customAcpUnavailable(binding, [])).toMatch(/removed/);
  expect(customAcpUnavailable(binding, [{ ...agent, enabled: false }])).toMatch(/disabled/);
  expect(customAcpUnavailable(binding, [{ ...agent, revision: "changed" }])).toMatch(/changed/);
  expect(customAcpUnavailable(binding, [agent])).toBeNull();
});
it("scopes empty authoritative catalogs to exact agent ID and launch revision", async () => {
  expect(typeof useCustomAcp.getState().probe).toBe("function");
  useCustomAcp.setState({ agents: [agent] });
  invoke.mockResolvedValueOnce(catalog as never);
  await useCustomAcp.getState().probe(agent.id);
  expect(useCustomAcp.getState().catalogs[acpCatalogKey(agent.id, agent.revision)]).toEqual(catalog);
  expect(invoke).toHaveBeenCalledExactlyOnceWith("acp_probe", { agentId: agent.id, cwd: null });
});
it("reads the exact live catalog after a configuration mutation instead of trusting an older acceptance snapshot", async () => {
  useCustomAcp.setState({ bindings: { restored: binding }, threadCatalogs: { restored: catalog } });
  const accepted = { ...catalog, current_model: "accepted" };
  const current = { ...catalog, current_model: '{"model":"opaque/Current","provider":"Case"}' };
  invoke.mockImplementation(async command => command === "acp_set_config" ? accepted : { catalog: current, live: true });
  const result = await useCustomAcp.getState().setConfig("restored", "Opaque/CONFIG", " Exact value ");
  expect(invoke).toHaveBeenCalledWith("acp_thread_catalog", { threadId: "restored" });
  expect(result.current_model).toBe(current.current_model);
  expect(useCustomAcp.getState().threadCatalogs.restored.current_model).toBe(current.current_model);
});
it.each(["model", "config"] as const)("F4: %s owns the per-thread gate against overlapping config/model calls until readback", async first => {
  useCustomAcp.setState({ bindings: { restored: binding }, threadCatalogs: { restored: catalog } });
  let acknowledge!: () => void;
  let readback!: (value: AcpThreadCatalog) => void;
  invoke.mockImplementation(async command => {
    if (command === "acp_set_config" || command === "agent_chat_set_model") return new Promise<void>(r => { acknowledge = r; });
    if (command === "acp_thread_catalog") return new Promise<AcpThreadCatalog>(r => { readback = r; });
    throw new Error(`Unexpected command: ${command}`);
  });
  const store = useCustomAcp.getState();
  const changing = first === "model" ? store.setModel("restored", " opaque/model ") : store.setConfig("restored", " opaque/config ", " opaque/value ");
  expect(useCustomAcp.getState().busy.restored).toBe(true);
  await expect(store.setModel("restored", "duplicate")).rejects.toThrow(/Wait/);
  await expect(store.setConfig("restored", "overlap", true)).rejects.toThrow(/Wait/);
  expect(useCustomAcp.getState().busy.restored).toBe(true);
  expect(invoke.mock.calls).toHaveLength(1);
  acknowledge();
  await vi.waitFor(() => expect(readback).toBeTypeOf("function"));
  await expect(store.setModel("restored", "during readback")).rejects.toThrow(/Wait/);
  expect(useCustomAcp.getState().busy.restored).toBe(true);
  const updated = { ...catalog, current_model: " opaque/model " };
  readback({ catalog: updated, live: true });
  await expect(changing).resolves.toEqual(updated);
  expect(useCustomAcp.getState().busy.restored).toBe(false);
  expect(invoke.mock.calls).toHaveLength(2);
  if (first === "model") expect(invoke).toHaveBeenCalledWith("agent_chat_set_model", { provider: "acp", threadId: "restored", model: " opaque/model " });
});
function deferredCatalogs() {
  const pending: { resolve: (value: AcpThreadCatalog) => void; reject: (reason: Error) => void }[] = [];
  invoke.mockImplementation(async command => command === "acp_thread_catalog" ? new Promise<AcpThreadCatalog>((resolve, reject) => pending.push({ resolve, reject })) : undefined);
  useCustomAcp.setState({ bindings: { restored: binding }, threadCatalogs: { restored: catalog }, reads: { restored: { ready: true, error: null } } });
  return pending;
}
it.each([true, false].flatMap(oldFails => [true, false].map(latestFails => ({ oldFails, latestFails }))))
("P1: settled latest outcome survives obsolete completion (oldFails=$oldFails latestFails=$latestFails)", async ({ oldFails, latestFails }) => {
  const pending = deferredCatalogs();
  const changing = useCustomAcp.getState().setModel("restored", " opaque/model ");
  const outcome = changing.then(value => ({ value, error: null }), error => ({ value: null, error: String(error) }));
  await vi.waitFor(() => expect(pending).toHaveLength(1));
  const newest = useCustomAcp.getState().refreshThread("restored");
  const latestOutcome = newest.catch(() => null);
  const current = { ...catalog, current_model: " exact/latest " };
  if (latestFails) pending[1].reject(new Error("latest failure"));
  else pending[1].resolve({ catalog: current, live: false });
  await latestOutcome;
  expect(useCustomAcp.getState().busy.restored).toBe(true);
  if (oldFails) pending[0].reject(new Error("obsolete failure"));
  else pending[0].resolve({ catalog: { ...catalog, current_model: "obsolete" }, live: true });
  expect(await outcome).toEqual(latestFails ? { value: null, error: "Error: latest failure" } : { value: current, error: null });
  expect(useCustomAcp.getState().reads.restored).toEqual(latestFails ? { ready: false, error: "Error: latest failure" } : { ready: true, error: null });
  expect(useCustomAcp.getState().busy.restored).toBe(false);
});
it.each([false, true])("P1: three generations retain the gate until final authority (failure=%s)", async latestFails => {
  const pending = deferredCatalogs();
  const changing = useCustomAcp.getState().setConfig("restored", " exact/config ", true);
  const outcome = changing.then(value => ({ value, error: null }), error => ({ value: null, error: String(error) }));
  await vi.waitFor(() => expect(pending).toHaveLength(1));
  const second = useCustomAcp.getState().refreshThread("restored").catch(() => null);
  pending[0].reject(new Error("obsolete first"));
  await Promise.resolve();
  const third = useCustomAcp.getState().refreshThread("restored").catch(() => null);
  pending[1].resolve({ catalog: { ...catalog, current_model: "obsolete second" }, live: true });
  await Promise.resolve(); await Promise.resolve();
  expect(useCustomAcp.getState().busy.restored).toBe(true);
  expect(useCustomAcp.getState().threadCatalogs.restored).toEqual(catalog);
  const current = { ...catalog, current_model: " exact/final " };
  if (latestFails) pending[2].reject(new Error("final failure"));
  else pending[2].resolve({ catalog: current, live: false });
  expect(await outcome).toEqual(latestFails ? { value: null, error: "Error: final failure" } : { value: current, error: null });
  await Promise.all([second, third]);
  expect(useCustomAcp.getState().busy.restored).toBe(false);
  expect(pending).toHaveLength(3);
});
it.each([false, true])("P1: reentrant catalog publication follows newer authority without self-await (failure=%s)", async latestFails => {
  const pending = deferredCatalogs();
  const intermediate = { ...catalog, current_model: "intermediate" };
  let newer: Promise<AcpCatalog | null> | undefined;
  const unsubscribe = useCustomAcp.subscribe(s => {
    if (!newer && s.threadCatalogs.restored === intermediate) newer = s.refreshThread("restored").catch(() => null);
  });
  try {
    const changing = useCustomAcp.getState().setModel("restored", " opaque/model ");
    const outcome = changing.then(value => ({ value, error: null }), error => ({ value: null, error: String(error) }));
    await vi.waitFor(() => expect(pending).toHaveLength(1));
    pending[0].resolve({ catalog: intermediate, live: true });
    await vi.waitFor(() => expect(pending).toHaveLength(2));
    expect(useCustomAcp.getState().busy.restored).toBe(true);
    const current = { ...catalog, current_model: null };
    if (latestFails) pending[1].reject(new Error("reentrant failure"));
    else pending[1].resolve({ catalog: current, live: false });
    expect(await outcome).toEqual(latestFails ? { value: null, error: "Error: reentrant failure" } : { value: current, error: null });
    await newer;
    expect(useCustomAcp.getState().busy.restored).toBe(false);
    expect(pending).toHaveLength(2);
  } finally { unsubscribe(); }
});
it.each([false, true])("P1: reentrant error publication keeps the mutation gate through replacement authority (failure=%s)", async latestFails => {
  const pending = deferredCatalogs();
  let newer: Promise<AcpCatalog | null> | undefined;
  const unsubscribe = useCustomAcp.subscribe(s => {
    if (!newer && s.reads.restored?.error === "Error: replaced failure") newer = s.refreshThread("restored").catch(() => null);
  });
  try {
    const changing = useCustomAcp.getState().setConfig("restored", " exact/config ", true);
    const outcome = changing.then(value => ({ value, error: null }), error => ({ value: null, error: String(error) }));
    await vi.waitFor(() => expect(pending).toHaveLength(1));
    pending[0].reject(new Error("replaced failure"));
    await vi.waitFor(() => expect(pending).toHaveLength(2));
    expect(useCustomAcp.getState().busy.restored).toBe(true);
    const current = { ...catalog, current_model: "replacement authority" };
    if (latestFails) pending[1].reject(new Error("replacement failure"));
    else pending[1].resolve({ catalog: current, live: false });
    expect(await outcome).toEqual(latestFails ? { value: null, error: "Error: replacement failure" } : { value: current, error: null });
    await newer;
    expect(useCustomAcp.getState().busy.restored).toBe(false);
  } finally {
    unsubscribe();
    for (const read of pending) read.resolve({ catalog, live: false });
  }
});

it.each([false, true])("P1: replaced binding's newer catalog fences obsolete identity/error (failure=%s)", async oldFails => {
  const pending = deferredCatalogs();
  const old = useCustomAcp.getState().refreshThread("restored");
  const oldOutcome = old.catch(() => null);
  const current = { ...catalog, current_model: "new binding" };
  useCustomAcp.setState({ bindings: { restored: { ...binding, session_id: "new session" } } });
  const newer = useCustomAcp.getState().refreshThread("restored");
  pending[1].resolve({ catalog: current, live: true });
  await newer;
  if (oldFails) pending[0].reject(new Error("old binding failed"));
  else pending[0].resolve({ catalog, live: false });
  expect(await oldOutcome).toEqual(current);
  expect(useCustomAcp.getState().reads.restored).toEqual({ ready: true, error: null });
  expect(useCustomAcp.getState().live.restored).toBe(true);
});
it("P1: a newer generation without read authority cannot fall back to cached readiness", async () => {
  const pending = deferredCatalogs();
  const fallback = invoke.getMockImplementation()!;
  let acknowledge!: () => void;
  invoke.mockImplementation(async command => command === "agent_chat_set_model" ? new Promise<void>(resolve => { acknowledge = resolve; }) : fallback(command));
  const old = useCustomAcp.getState().refreshThread("restored");
  const oldOutcome = old.then(value => ({ value, error: null }), error => ({ value: null, error: String(error) }));
  const changing = useCustomAcp.getState().setModel("restored", " opaque/model ");
  pending[0].resolve({ catalog, live: true });
  expect(await oldOutcome).toEqual({ value: null, error: "Error: Custom agent catalog read superseded before authoritative readback." });
  expect(useCustomAcp.getState().reads.restored.ready).toBe(false);
  acknowledge();
  await vi.waitFor(() => expect(pending).toHaveLength(2));
  expect(useCustomAcp.getState().busy.restored).toBe(true);
  pending[1].resolve({ catalog, live: false });
  await changing;
  expect(useCustomAcp.getState().reads.restored.ready).toBe(true);
});

it("requires an explicit saved custom agent before a new ACP session can launch", async () => {
  await expect(agentChatStartSession("pane", "acp" as never, { thread_id: "new-thread", cwd: "/repo", model: null, resume_cursor: null, permission_mode: null, effort: null, context_window: null, additional_directories: [], env: null })).rejects.toThrow("Choose a custom agent");
  expect(invoke).not.toHaveBeenCalledWith("agent_chat_start_session", expect.anything());
});
it("verifies and fixes the new thread binding after launch while preserving null default and opaque instance IDs", async () => {
  const id = " ID /Case:opaque ";
  useCustomAcp.getState().select("launched", id);
  let started = false;
  invoke.mockImplementation(async (command: string) => {
    if (command === "acp_binding") return started ? { ...binding, thread_id: "launched", agent_id: id } as never : null;
    if (command === "agent_chat_start_session") { started = true; return "launched"; }
    return null;
  });
  await agentChatStartSession("pane", "acp", { thread_id: "launched", cwd: "/repo", model: null, resume_cursor: null, permission_mode: "autoapprove", additional_directories: [], env: null });
  expect(invoke).toHaveBeenCalledWith("agent_chat_start_session", expect.objectContaining({ provider: "acp", input: expect.objectContaining({ model: null, permission_mode: null, extra: { acp_agent_id: id } }) }));
  expect(useCustomAcp.getState().bindings.launched?.agent_id).toBe(id);
});
it("rejects an explicit instance mismatch against the backend binding instead of rewriting its identity", async () => {
  invoke.mockResolvedValue(binding as never);
  await expect(agentChatStartSession("pane", "acp", { thread_id: binding.thread_id, cwd: "/repo", model: null, resume_cursor: { resume: "opaque" }, permission_mode: null, additional_directories: [], env: null, extra: { acp_agent_id: "other" } })).rejects.toThrow(/different custom agent/);
  expect(invoke).not.toHaveBeenCalledWith("agent_chat_start_session", expect.anything());
});
