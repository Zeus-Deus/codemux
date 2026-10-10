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
  useCustomAcp.setState({ selections: {}, bindings: {}, agents: null, catalogs: {}, threadCatalogs: {}, errors: {}, restored: {}, busy: {}, live: {} });
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
