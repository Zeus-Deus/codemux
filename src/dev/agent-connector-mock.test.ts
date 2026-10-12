import { afterAll, afterEach, beforeAll, expect, it, vi } from "vitest";
import { listen } from "@tauri-apps/api/event";
import type { AgentConnectorStatus } from "@/tauri/types";
import { createAgentConnectorMock } from "./agent-connector-mock";

type Invoke = (command: string, args?: Record<string, unknown>) => Promise<unknown>;
let invoke: Invoke;
beforeAll(async () => {
  vi.useFakeTimers();
  await import("./tauri-mock");
  invoke = (window as unknown as { __TAURI_INTERNALS__: { invoke: Invoke } }).__TAURI_INTERNALS__.invoke;
});
afterAll(() => { vi.clearAllTimers(); vi.useRealTimers(); });
afterEach(() => { vi.clearAllMocks(); });

it("routes disabled-by-default connector configuration without enabling Remote Access", async () => {
  const before = await invoke("web_remote_status");
  const status = await invoke("agent_connector_status") as AgentConnectorStatus;
  expect(status).toMatchObject({ enabled: false, publicOrigin: null, pending: [], clients: [], localCommand: "/usr/bin/codemux mcp" });
  const changed = vi.fn();
  const unlisten = await listen("agent-connector-changed", (event) => changed(event.payload));
  const written = await invoke("agent_connector_set_config", { enabled: true, publicOrigin: "https://connector.example.test" });
  expect(written).toMatchObject({ enabled: true, publicOrigin: "https://connector.example.test" });
  expect(await invoke("web_remote_status")).toEqual(before);
  expect(changed).toHaveBeenCalledWith(null);
  unlisten();
});

it("emits sanitized changes when the existing listener is turned off", async () => {
  const changed = vi.fn();
  const unlisten = await listen("agent-connector-changed", (event) => changed(event.payload));
  await invoke("web_remote_disable");
  expect((await invoke("agent_connector_status") as AgentConnectorStatus).listenerRunning).toBe(false);
  expect(changed).toHaveBeenCalledWith(null);
  unlisten();
});

it("injects synthetic requests and routes approve, deny and revoke with read-back", async () => {
  const hook = (window as unknown as { __codemuxAgentConnectorMock: ReturnType<typeof createAgentConnectorMock> }).__codemuxAgentConnectorMock;
  expect(hook).toBeDefined();
  await invoke("web_remote_set_config", { lanEnabled: true, bindScope: "loopback" });
  await invoke("web_remote_enable");
  await invoke("agent_connector_set_config", { enabled: true, publicOrigin: null });
  const changes = vi.fn();
  const unlisten = await listen("agent-connector-changed", (event) => changes(event.payload));
  const denied = hook.addPendingClient("Denied example assistant");
  await invoke("agent_connector_deny", { requestId: denied });
  expect((await invoke("agent_connector_status") as AgentConnectorStatus).pending[0]).toMatchObject({ phase: "denied", access: null });
  hook.continuePendingClient(denied);
  expect((await invoke("agent_connector_status") as AgentConnectorStatus).pending).toEqual([]);
  const approved = hook.addPendingClient("Approved example assistant");
  await invoke("agent_connector_approve", { requestId: approved, access: "supervised" });
  const awaiting = await invoke("agent_connector_status") as AgentConnectorStatus;
  expect(awaiting.pending[0]).toMatchObject({ phase: "approved_awaiting_client", access: "supervised" });
  expect(awaiting.clients).toEqual([]);
  hook.continuePendingClient(approved);
  expect((await invoke("agent_connector_status") as AgentConnectorStatus).clients).toEqual([]);
  hook.exchangePendingClient(approved);
  const status = await invoke("agent_connector_status") as AgentConnectorStatus;
  expect(status.pending).toEqual([]);
  expect(status.clients).toHaveLength(1);
  expect(status.clients[0]).toMatchObject({ clientName: "Approved example assistant", access: "supervised" });
  expect(status.mcpUrl).toMatch(/^http:\/\/127\.0\.0\.1:\d+\/mcp$/);
  await invoke("agent_connector_revoke", { clientId: status.clients[0].id });
  expect((await invoke("agent_connector_status") as AgentConnectorStatus).clients).toEqual([]);
  expect(changes.mock.calls).toHaveLength(8);
  expect(changes.mock.calls.every(([payload]) => payload === null)).toBe(true);
  unlisten();
});

it("requires explicit synthetic Continue and exchange before showing a one-hour grant", () => {
  const mock = createAgentConnectorMock(() => ({ running: true, port: 4377 }), vi.fn());
  // These browser-QA hooks simulate phases only, never native OAuth proof.
  const qa = mock;
  expect(qa.continuePendingClient).toBeTypeOf("function");
  expect(qa.exchangePendingClient).toBeTypeOf("function");
  mock.handlers.agent_connector_set_config({ enabled: true, publicOrigin: null });
  const id = mock.addPendingClient();
  qa.continuePendingClient(id);
  expect((mock.handlers.agent_connector_status({}) as AgentConnectorStatus).pending).toHaveLength(1);
  expect(() => qa.exchangePendingClient(id)).toThrow();
  mock.handlers.agent_connector_approve({ requestId: id, access: "supervised" });
  qa.continuePendingClient(id);
  const continued = mock.handlers.agent_connector_status({}) as AgentConnectorStatus;
  expect(continued.pending).toEqual([]);
  expect(continued.clients).toEqual([]);
  expect(() => mock.handlers.agent_connector_deny({ requestId: id })).toThrow(/no longer/i);
  qa.exchangePendingClient(id);
  const complete = mock.handlers.agent_connector_status({}) as AgentConnectorStatus;
  expect(complete.clients).toHaveLength(1);
  expect(complete.clients[0].access).toBe("supervised");
  expect(Date.parse(complete.clients[0].expiresAt) - Date.parse(complete.clients[0].createdAt)).toBe(3_600_000);
  expect(() => qa.exchangePendingClient(id)).toThrow();
  const denied = mock.addPendingClient();
  mock.handlers.agent_connector_deny({ requestId: denied });
  qa.continuePendingClient(denied);
  expect((mock.handlers.agent_connector_status({}) as AgentConnectorStatus).pending).toEqual([]);
  expect(() => qa.exchangePendingClient(denied)).toThrow();
  expect((mock.handlers.agent_connector_status({}) as AgentConnectorStatus).clients).toHaveLength(1);
});

it.each(["status", "approve", "complete", "exchange"] as const)("invalidates obsolete loopback authorization before synthetic %s after a port rebind", (boundary) => {
  let port = 4377;
  const mock = createAgentConnectorMock(() => ({ running: true, port }), vi.fn());
  mock.handlers.agent_connector_set_config({ enabled: true, publicOrigin: null });
  const id = mock.addPendingClient();
  if (boundary === "complete" || boundary === "exchange") mock.handlers.agent_connector_approve({ requestId: id, access: "read_only" });
  if (boundary === "exchange") mock.continuePendingClient(id);
  port = 5432;
  if (boundary === "status") expect((mock.handlers.agent_connector_status({}) as AgentConnectorStatus).pending).toEqual([]);
  else if (boundary === "approve") expect(() => mock.handlers.agent_connector_approve({ requestId: id, access: "read_only" })).toThrow(/no longer/i);
  else if (boundary === "complete") expect(() => mock.continuePendingClient(id)).toThrow(/no longer/i);
  else expect(() => mock.exchangePendingClient(id)).toThrow();
  port = 4377;
  expect((mock.handlers.agent_connector_status({}) as AgentConnectorStatus).pending).toEqual([]);
  expect(() => mock.exchangePendingClient(id)).toThrow();
});

it("retains unchanged public bindings across port changes and offline denial", () => {
  let port = 4377;
  let running = true;
  const mock = createAgentConnectorMock(() => ({ running, port }), vi.fn());
  mock.handlers.agent_connector_set_config({ enabled: true, publicOrigin: "https://mcp.example" });
  const id = mock.addPendingClient();
  port = 5432;
  running = false;
  expect((mock.handlers.agent_connector_status({}) as AgentConnectorStatus).pending[0].id).toBe(id);
  mock.handlers.agent_connector_deny({ requestId: id });
  expect((mock.handlers.agent_connector_status({}) as AgentConnectorStatus).pending[0].phase).toBe("denied");
});

it.each(["disable", "origin change"] as const)("clears every outstanding authorization on %s without pretending to revoke minted grants", (change) => {
  const mock = createAgentConnectorMock(() => ({ running: true, port: 4377 }), vi.fn());
  mock.handlers.agent_connector_set_config({ enabled: true, publicOrigin: null });
  const granted = mock.addPendingClient("Already connected");
  mock.handlers.agent_connector_approve({ requestId: granted, access: "read_only" });
  mock.continuePendingClient(granted);
  mock.exchangePendingClient(granted);
  const undecided = mock.addPendingClient("Undecided");
  const approved = mock.addPendingClient("Approved");
  mock.handlers.agent_connector_approve({ requestId: approved, access: "supervised" });
  const denied = mock.addPendingClient("Denied");
  mock.handlers.agent_connector_deny({ requestId: denied });
  const continued = mock.addPendingClient("Awaiting exchange");
  mock.handlers.agent_connector_approve({ requestId: continued, access: "full_access" });
  mock.continuePendingClient(continued);
  mock.handlers.agent_connector_set_config({ enabled: change !== "disable", publicOrigin: change === "disable" ? null : "https://new.example.test" });
  const status = mock.handlers.agent_connector_status({}) as AgentConnectorStatus;
  expect(status.pending).toEqual([]);
  expect(status.clients).toHaveLength(1);
  mock.handlers.agent_connector_set_config({ enabled: true, publicOrigin: status.publicOrigin });
  for (const requestId of [undecided, approved, denied]) {
    expect(() => mock.continuePendingClient(requestId)).toThrow(/no longer/i);
    expect(() => mock.handlers.agent_connector_deny({ requestId })).toThrow(/no longer/i);
  }
  expect(() => mock.exchangePendingClient(continued)).toThrow();
});

it.each(["pending", "exchange", "grant"] as const)("expires synthetic %s state at the native lifetime boundary", (stage) => {
  const mock = createAgentConnectorMock(() => ({ running: true, port: 4377 }), vi.fn());
  mock.handlers.agent_connector_set_config({ enabled: true, publicOrigin: null });
  const id = mock.addPendingClient();
  if (stage !== "pending") {
    mock.handlers.agent_connector_approve({ requestId: id, access: "read_only" });
    mock.continuePendingClient(id);
  }
  if (stage === "grant") mock.exchangePendingClient(id);
  const start = Date.now();
  const ttl = stage === "pending" ? 300_000 : stage === "exchange" ? 60_000 : 3_600_000;
  vi.setSystemTime(start + ttl - 1);
  const before = mock.handlers.agent_connector_status({}) as AgentConnectorStatus;
  if (stage === "pending") expect(before.pending).toHaveLength(1);
  if (stage === "grant") expect(before.clients).toHaveLength(1);
  vi.setSystemTime(start + ttl);
  if (stage === "pending") expect(() => mock.handlers.agent_connector_deny({ requestId: id })).toThrow(/no longer/i);
  if (stage === "exchange") expect(() => mock.exchangePendingClient(id)).toThrow();
  const expired = mock.handlers.agent_connector_status({}) as AgentConnectorStatus;
  expect(expired.pending).toEqual([]);
  expect(expired.clients).toEqual([]);
});

it.each(["read_only", "supervised", "full_access"] as const)("retains approval and its selected %s ceiling without minting or allowing repeated decisions", (access) => {
  const changed = vi.fn();
  const mock = createAgentConnectorMock(() => ({ running: true, port: 4377 }), changed);
  mock.handlers.agent_connector_set_config({ enabled: true, publicOrigin: null });
  const id = mock.addPendingClient();
  mock.handlers.agent_connector_approve({ requestId: id, access });
  const status = mock.handlers.agent_connector_status({}) as AgentConnectorStatus;
  expect(status.pending).toEqual([expect.objectContaining({ id, phase: "approved_awaiting_client", access })]);
  expect(status.clients).toEqual([]);
  changed.mockClear();
  expect(() => mock.handlers.agent_connector_approve({ requestId: id, access: "full_access" })).toThrow(/already decided/i);
  expect(() => mock.handlers.agent_connector_deny({ requestId: id })).toThrow(/already decided/i);
  expect(changed).not.toHaveBeenCalled();
});

it("retains denied requests offline until explicit fixture completion without repeat decisions", () => {
  let running = true;
  const changed = vi.fn();
  const mock = createAgentConnectorMock(() => ({ running, port: 4377 }), changed);
  mock.handlers.agent_connector_set_config({ enabled: true, publicOrigin: null });
  const id = mock.addPendingClient();
  running = false;
  mock.handlers.agent_connector_deny({ requestId: id });
  const status = mock.handlers.agent_connector_status({}) as AgentConnectorStatus;
  expect(status.pending).toEqual([expect.objectContaining({ id, phase: "denied", access: null })]);
  expect(status.clients).toEqual([]);
  changed.mockClear();
  expect(() => mock.handlers.agent_connector_deny({ requestId: id })).toThrow(/already decided/i);
  expect(changed).not.toHaveBeenCalled();
});

it("does not add a double slash to a public-origin MCP endpoint", async () => {
  await invoke("agent_connector_set_config", { enabled: true, publicOrigin: "https://connector.example.test/" });
  expect((await invoke("agent_connector_status") as AgentConnectorStatus).mcpUrl).toBe("https://connector.example.test/mcp");
});

it.each([
  "https://user:pass@example.test", "https://example.test/path", "https://example.test?", "http://example.test",
  "https://example.test#", "https://example.test/../", "https://example.test\\\\path",
  " https://example.test", "https://example.test ",
])("rejects invalid synthetic public origins: %s", async (publicOrigin) => {
  const before = await invoke("agent_connector_status");
  await expect(invoke("agent_connector_set_config", { enabled: true, publicOrigin })).rejects.toThrow(/HTTPS origin/);
  expect(await invoke("agent_connector_status")).toEqual(before);
});

it.each([31, 0, 1, 127])("rejects raw origin control character %i without storing configuration", (code) => {
  const changed = vi.fn();
  const mock = createAgentConnectorMock(() => ({ running: true, port: 4377 }), changed);
  mock.handlers.agent_connector_set_config({ enabled: false, publicOrigin: "https://saved.example.test" });
  const before = mock.handlers.agent_connector_status({});
  changed.mockClear();
  const publicOrigin = "https://connector.example.test" + String.fromCharCode(code);
  expect(() => mock.handlers.agent_connector_set_config({ enabled: true, publicOrigin })).toThrow(/HTTPS origin/);
  expect(mock.handlers.agent_connector_status({})).toEqual(before);
  expect(changed).not.toHaveBeenCalled();
});

it.each([31, 0, 1, 127])("rejects raw callback control character %i without adding a pending request", (code) => {
  const changed = vi.fn();
  const mock = createAgentConnectorMock(() => ({ running: true, port: 4377 }), changed);
  mock.handlers.agent_connector_set_config({ enabled: true, publicOrigin: null });
  const before = mock.handlers.agent_connector_status({});
  changed.mockClear();
  const callbackOrigin = "https://assistant.example.test" + String.fromCharCode(code);
  expect(() => mock.addPendingClient("Example assistant", callbackOrigin)).toThrow(/HTTPS callback origin/);
  expect(mock.handlers.agent_connector_status({})).toEqual(before);
  expect(changed).not.toHaveBeenCalled();
});
