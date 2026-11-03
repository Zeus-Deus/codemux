import { afterAll, afterEach, beforeAll, expect, it, vi } from "vitest";
import { listen } from "@tauri-apps/api/event";
import type { AgentConnectorIssuedToken, AgentConnectorStatus } from "@/tauri/types";
import { createAgentConnectorMock, MOCK_CONNECTOR_PORT, MOCK_SIGN_IN_LIFETIME_MS } from "./agent-connector-mock";

type Invoke = (command: string, args?: Record<string, unknown>) => Promise<unknown>;
let invoke: Invoke;
beforeAll(async () => {
  vi.useFakeTimers();
  await import("./tauri-mock");
  invoke = (window as unknown as { __TAURI_INTERNALS__: { invoke: Invoke } }).__TAURI_INTERNALS__.invoke;
});
afterAll(() => { vi.clearAllTimers(); vi.useRealTimers(); });
afterEach(() => { vi.clearAllMocks(); });

type Mock = ReturnType<typeof createAgentConnectorMock>;
const read = (mock: Mock) => mock.handlers.agent_connector_status({}) as AgentConnectorStatus;
/** A fresh mock with network connections on and its listener bound. */
function listening(changed = vi.fn()) {
  const mock = createAgentConnectorMock(changed);
  mock.handlers.agent_connector_set_config({ enabled: true, publicOrigin: null });
  return mock;
}
function signIn(mock: Mock, name = "Already connected", access = "read_only") {
  const id = mock.addPendingClient(name);
  mock.handlers.agent_connector_approve({ requestId: id, access });
  mock.continuePendingClient(id);
  mock.exchangePendingClient(id);
  return id;
}

it("routes disabled-by-default connector configuration without enabling Remote Access", async () => {
  const before = await invoke("web_remote_status");
  const status = await invoke("agent_connector_status") as AgentConnectorStatus;
  expect(status).toMatchObject({
    enabled: false, publicOrigin: null, pending: [], clients: [], listenerRunning: false, port: MOCK_CONNECTOR_PORT,
    bindError: null, localUrl: null, mcpUrl: null, localExecutable: "/usr/bin/codemux", localCommand: "/usr/bin/codemux mcp",
  });
  const changed = vi.fn();
  const unlisten = await listen("agent-connector-changed", (event) => changed(event.payload));
  const written = await invoke("agent_connector_set_config", { enabled: true, publicOrigin: "https://connector.example.test" });
  expect(written).toMatchObject({ enabled: true, publicOrigin: "https://connector.example.test", listenerRunning: true, revokedGrants: 0 });
  expect(await invoke("web_remote_status")).toEqual(before);
  expect(changed).toHaveBeenCalledWith(null);
  unlisten();
});

it("listener belongs to the connector: web-remote enable and disable leave it unchanged", async () => {
  await invoke("agent_connector_set_config", { enabled: true, publicOrigin: null });
  const changed = vi.fn();
  const unlisten = await listen("agent-connector-changed", (event) => changed(event.payload));
  const before = await invoke("agent_connector_status") as AgentConnectorStatus;
  expect(before).toMatchObject({ listenerRunning: true, localUrl: `http://127.0.0.1:${MOCK_CONNECTOR_PORT}/mcp` });
  await invoke("web_remote_set_config", { lanEnabled: true, bindScope: "loopback" });
  await invoke("web_remote_enable");
  await invoke("web_remote_disable");
  expect(await invoke("agent_connector_status")).toEqual(before);
  expect(changed).not.toHaveBeenCalled();
  unlisten();
});

it("injects synthetic requests and routes approve, deny and revoke with read-back", async () => {
  const hook = (window as unknown as { __codemuxAgentConnectorMock: Mock }).__codemuxAgentConnectorMock;
  expect(hook).toBeDefined();
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
  expect(status.clients[0]).toMatchObject({ clientName: "Approved example assistant", access: "supervised", kind: "oauth" });
  expect(status.mcpUrl).toMatch(/^http:\/\/127\.0\.0\.1:\d+\/mcp$/);
  await invoke("agent_connector_revoke", { clientId: status.clients[0].id });
  expect((await invoke("agent_connector_status") as AgentConnectorStatus).clients).toEqual([]);
  expect(changes.mock.calls).toHaveLength(8);
  expect(changes.mock.calls.every(([payload]) => payload === null)).toBe(true);
  unlisten();
});

it("requires explicit synthetic Continue and exchange before showing a renewing sign-in", () => {
  const mock = listening();
  const id = mock.addPendingClient();
  mock.continuePendingClient(id);
  expect(read(mock).pending).toHaveLength(1);
  expect(() => mock.exchangePendingClient(id)).toThrow();
  mock.handlers.agent_connector_approve({ requestId: id, access: "supervised" });
  mock.continuePendingClient(id);
  const continued = read(mock);
  expect(continued.pending).toEqual([]);
  expect(continued.clients).toEqual([]);
  expect(() => mock.handlers.agent_connector_deny({ requestId: id })).toThrow(/no longer/i);
  mock.exchangePendingClient(id);
  const [client] = read(mock).clients;
  expect(client).toMatchObject({ access: "supervised", kind: "oauth", label: null, renews: true, callbackOrigin: "https://assistant.example.test" });
  expect(Date.parse(client.expiresAt) - Date.parse(client.createdAt)).toBe(MOCK_SIGN_IN_LIFETIME_MS);
  expect(() => mock.exchangePendingClient(id)).toThrow();
  const denied = mock.addPendingClient();
  mock.handlers.agent_connector_deny({ requestId: denied });
  mock.continuePendingClient(denied);
  expect(read(mock).pending).toEqual([]);
  expect(() => mock.exchangePendingClient(denied)).toThrow();
  expect(read(mock).clients).toHaveLength(1);
});

it("marks a request from a previously approved assistant as reconnecting", () => {
  const mock = listening();
  expect(read(mock).pending).toEqual([]);
  const first = mock.addPendingClient("Returning assistant");
  expect(read(mock).pending[0]).toMatchObject({ id: first, reconnecting: false });
  mock.handlers.agent_connector_approve({ requestId: first, access: "read_only" });
  mock.continuePendingClient(first);
  mock.exchangePendingClient(first);
  const again = mock.addPendingClient("Returning assistant");
  const stranger = mock.addPendingClient("New assistant");
  const pending = read(mock).pending;
  expect(pending.find((request) => request.id === again)?.reconnecting).toBe(true);
  expect(pending.find((request) => request.id === stranger)?.reconnecting).toBe(false);
});

it.each(["status", "approve", "complete", "exchange"] as const)("invalidates obsolete loopback authorization before synthetic %s after a port change", (boundary) => {
  const mock = listening();
  signIn(mock);
  const id = mock.addPendingClient();
  if (boundary === "complete" || boundary === "exchange") mock.handlers.agent_connector_approve({ requestId: id, access: "read_only" });
  if (boundary === "exchange") mock.continuePendingClient(id);
  const moved = mock.handlers.agent_connector_set_config({ enabled: true, publicOrigin: null, port: 5432 }) as AgentConnectorStatus;
  // A loopback issuer names the port, so every grant bound to the old one ends.
  expect(moved).toMatchObject({ port: 5432, revokedGrants: 1, clients: [], mcpUrl: "http://127.0.0.1:5432/mcp" });
  if (boundary === "status") expect(read(mock).pending).toEqual([]);
  else if (boundary === "approve") expect(() => mock.handlers.agent_connector_approve({ requestId: id, access: "read_only" })).toThrow(/no longer/i);
  else if (boundary === "complete") expect(() => mock.continuePendingClient(id)).toThrow(/no longer/i);
  else expect(() => mock.exchangePendingClient(id)).toThrow();
  mock.handlers.agent_connector_set_config({ enabled: true, publicOrigin: null, port: MOCK_CONNECTOR_PORT });
  expect(read(mock).pending).toEqual([]);
  expect(read(mock).clients).toEqual([]);
  expect(() => mock.exchangePendingClient(id)).toThrow();
});

it("retains unchanged public bindings across port changes and offline denial", () => {
  const mock = createAgentConnectorMock(vi.fn());
  mock.handlers.agent_connector_set_config({ enabled: true, publicOrigin: "https://mcp.example" });
  signIn(mock);
  const id = mock.addPendingClient();
  const moved = mock.handlers.agent_connector_set_config({ enabled: true, publicOrigin: "https://mcp.example", port: 5432 }) as AgentConnectorStatus;
  expect(moved).toMatchObject({ revokedGrants: 0, mcpUrl: "https://mcp.example/mcp", localUrl: "http://127.0.0.1:5432/mcp" });
  mock.failListener("bind 127.0.0.1:5432: Address already in use");
  expect(read(mock).pending[0].id).toBe(id);
  expect(read(mock).clients).toHaveLength(1);
  mock.handlers.agent_connector_deny({ requestId: id });
  expect(read(mock).pending[0].phase).toBe("denied");
});

it.each(["disable", "origin change"] as const)("clears every outstanding authorization on %s, and revokes grants only when the address changes", (change) => {
  const mock = listening();
  signIn(mock);
  const undecided = mock.addPendingClient("Undecided");
  const approved = mock.addPendingClient("Approved");
  mock.handlers.agent_connector_approve({ requestId: approved, access: "supervised" });
  const denied = mock.addPendingClient("Denied");
  mock.handlers.agent_connector_deny({ requestId: denied });
  const continued = mock.addPendingClient("Awaiting exchange");
  mock.handlers.agent_connector_approve({ requestId: continued, access: "full_access" });
  mock.continuePendingClient(continued);
  const written = mock.handlers.agent_connector_set_config({ enabled: change !== "disable", publicOrigin: change === "disable" ? null : "https://new.example.test" }) as AgentConnectorStatus;
  const status = read(mock);
  expect(status.pending).toEqual([]);
  // Turning network connections off pauses grants; a new address ends them.
  expect(status.clients).toHaveLength(change === "disable" ? 1 : 0);
  expect(written.revokedGrants).toBe(change === "disable" ? 0 : 1);
  mock.handlers.agent_connector_set_config({ enabled: true, publicOrigin: status.publicOrigin });
  for (const requestId of [undecided, approved, denied]) {
    expect(() => mock.continuePendingClient(requestId)).toThrow(/no longer/i);
    expect(() => mock.handlers.agent_connector_deny({ requestId })).toThrow(/no longer/i);
  }
  expect(() => mock.exchangePendingClient(continued)).toThrow();
});

it.each(["pending", "exchange", "grant"] as const)("expires synthetic %s state at the native lifetime boundary", (stage) => {
  const mock = listening();
  const id = mock.addPendingClient();
  if (stage !== "pending") {
    mock.handlers.agent_connector_approve({ requestId: id, access: "read_only" });
    mock.continuePendingClient(id);
  }
  if (stage === "grant") mock.exchangePendingClient(id);
  const start = Date.now();
  const ttl = stage === "pending" ? 300_000 : stage === "exchange" ? 60_000 : MOCK_SIGN_IN_LIFETIME_MS;
  vi.setSystemTime(start + ttl - 1);
  const before = read(mock);
  if (stage === "pending") expect(before.pending).toHaveLength(1);
  if (stage === "grant") expect(before.clients).toHaveLength(1);
  vi.setSystemTime(start + ttl);
  if (stage === "pending") expect(() => mock.handlers.agent_connector_deny({ requestId: id })).toThrow(/no longer/i);
  if (stage === "exchange") expect(() => mock.exchangePendingClient(id)).toThrow();
  const expired = read(mock);
  expect(expired.pending).toEqual([]);
  expect(expired.clients).toEqual([]);
});

it.each(["read_only", "supervised", "full_access"] as const)("retains approval and its selected %s ceiling without minting or allowing repeated decisions", (access) => {
  const changed = vi.fn();
  const mock = listening(changed);
  const id = mock.addPendingClient();
  mock.handlers.agent_connector_approve({ requestId: id, access });
  const status = read(mock);
  expect(status.pending).toEqual([expect.objectContaining({ id, phase: "approved_awaiting_client", access })]);
  expect(status.clients).toEqual([]);
  changed.mockClear();
  expect(() => mock.handlers.agent_connector_approve({ requestId: id, access: "full_access" })).toThrow(/already decided/i);
  expect(() => mock.handlers.agent_connector_deny({ requestId: id })).toThrow(/already decided/i);
  expect(changed).not.toHaveBeenCalled();
});

it("retains denied requests offline until explicit fixture completion without repeat decisions", () => {
  const changed = vi.fn();
  const mock = listening(changed);
  const id = mock.addPendingClient();
  mock.failListener("bind 127.0.0.1:4378: Address already in use");
  mock.handlers.agent_connector_deny({ requestId: id });
  const status = read(mock);
  expect(status.pending).toEqual([expect.objectContaining({ id, phase: "denied", access: null })]);
  expect(status.clients).toEqual([]);
  changed.mockClear();
  expect(() => mock.handlers.agent_connector_deny({ requestId: id })).toThrow(/already decided/i);
  expect(changed).not.toHaveBeenCalled();
});

it("failListener reports a sanitized error, stops the URL and blocks approval", () => {
  const changed = vi.fn();
  const mock = listening(changed);
  const id = mock.addPendingClient();
  changed.mockClear();
  mock.failListener("bind 127.0.0.1:4378: Address already in use");
  expect(changed).toHaveBeenCalledTimes(1);
  expect(read(mock)).toMatchObject({ listenerRunning: false, bindError: "bind 127.0.0.1:4378: Address already in use", localUrl: null, mcpUrl: null, port: 4378 });
  expect(() => mock.handlers.agent_connector_approve({ requestId: id, access: "read_only" })).toThrow(/unavailable/i);
  // Retrying the same busy port keeps failing; another port binds.
  mock.handlers.agent_connector_set_config({ enabled: true, publicOrigin: null });
  expect(read(mock).bindError).toMatch(/in use/);
  mock.handlers.agent_connector_set_config({ enabled: true, publicOrigin: null, port: 5000 });
  expect(read(mock)).toMatchObject({ listenerRunning: true, bindError: null, port: 5000, localUrl: "http://127.0.0.1:5000/mcp" });
  // A failure is only an error while network connections are on.
  mock.failListener("bind 127.0.0.1:5000: Address already in use");
  mock.handlers.agent_connector_set_config({ enabled: false, publicOrigin: null });
  expect(read(mock)).toMatchObject({ listenerRunning: false, bindError: null });
});

it.each([80, 1023, 65536, 4378.5, "5000", null])("rejects port %s without changing configuration", (port) => {
  const changed = vi.fn();
  const mock = listening(changed);
  const before = read(mock);
  changed.mockClear();
  expect(() => mock.handlers.agent_connector_set_config({ enabled: true, publicOrigin: null, port })).toThrow(/Invalid port/);
  expect(read(mock)).toEqual(before);
  expect(changed).not.toHaveBeenCalled();
});

it("issues a synthetic token once and lists it as a token client", () => {
  const changed = vi.fn();
  const mock = listening(changed);
  changed.mockClear();
  const issued = mock.handlers.agent_connector_issue({ label: "  hermes-bot ", access: "supervised" }) as AgentConnectorIssuedToken;
  expect(changed).toHaveBeenCalledTimes(1);
  expect(issued).toMatchObject({ label: "hermes-bot", access: "supervised", replaced: false,
    mcpUrl: "http://127.0.0.1:4378/mcp", localUrl: "http://127.0.0.1:4378/mcp" });
  expect(issued.token).toContain("not-a-credential");
  expect(Date.parse(issued.expiresAt) - Date.now()).toBe(30 * 86_400_000);
  const status = read(mock);
  expect(status.clients).toEqual([expect.objectContaining({ id: issued.clientId, kind: "token", clientName: "hermes-bot",
    label: "hermes-bot", callbackOrigin: null, access: "supervised", renews: false, expiresAt: issued.expiresAt })]);
  expect(JSON.stringify(status)).not.toContain(issued.token);
  // The same label (in any case) rotates that bot's token and keeps its client id.
  const again = mock.handlers.agent_connector_issue({ label: "Hermes-Bot", access: "read_only", expiresInHours: 48 }) as AgentConnectorIssuedToken;
  expect(again).toMatchObject({ clientId: issued.clientId, replaced: true, access: "read_only" });
  expect(again.token).not.toBe(issued.token);
  expect(Date.parse(again.expiresAt) - Date.now()).toBe(48 * 3_600_000);
  expect(read(mock).clients).toHaveLength(1);
  expect(JSON.stringify(read(mock))).not.toContain("not-a-credential");
});

it.each([
  [{ label: "", access: "supervised" }, /label/],
  [{ label: " -leading", access: "supervised" }, /label/],
  [{ label: "x".repeat(65), access: "supervised" }, /label/],
  [{ label: "line\nbreak", access: "supervised" }, /label/],
  [{ label: "bot", access: "admin" }, /access/i],
  [{ label: "bot", access: "supervised", expiresInHours: 0 }, /expiry/],
  [{ label: "bot", access: "supervised", expiresInHours: 365 * 24 + 1 }, /expiry/],
] as const)("refuses an invalid token request %j without listing a client", (args, error) => {
  const mock = listening();
  expect(() => mock.handlers.agent_connector_issue({ ...args })).toThrow(error);
  expect(read(mock).clients).toEqual([]);
});

it("requires network connections, not a bound listener, to issue a token", () => {
  const mock = createAgentConnectorMock(vi.fn());
  expect(() => mock.handlers.agent_connector_issue({ label: "bot", access: "supervised" })).toThrow(/connector enable/);
  mock.handlers.agent_connector_set_config({ enabled: true, publicOrigin: null });
  mock.failListener("bind 127.0.0.1:4378: Address already in use");
  const issued = mock.handlers.agent_connector_issue({ label: "bot", access: "supervised" }) as AgentConnectorIssuedToken;
  expect(issued.mcpUrl).toBe("http://127.0.0.1:4378/mcp");
});

it("revokes every sign-in and token at once but keeps pending requests", () => {
  const changed = vi.fn();
  const mock = listening(changed);
  signIn(mock);
  mock.handlers.agent_connector_issue({ label: "hermes-bot", access: "supervised" });
  const waiting = mock.addPendingClient("Waiting");
  changed.mockClear();
  mock.handlers.agent_connector_revoke_all({});
  expect(changed).toHaveBeenCalledTimes(1);
  expect(read(mock).clients).toEqual([]);
  expect(read(mock).pending.map((request) => request.id)).toEqual([waiting]);
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
  const mock = createAgentConnectorMock(changed);
  mock.handlers.agent_connector_set_config({ enabled: false, publicOrigin: "https://saved.example.test" });
  const before = read(mock);
  changed.mockClear();
  const publicOrigin = "https://connector.example.test" + String.fromCharCode(code);
  expect(() => mock.handlers.agent_connector_set_config({ enabled: true, publicOrigin })).toThrow(/HTTPS origin/);
  expect(read(mock)).toEqual(before);
  expect(changed).not.toHaveBeenCalled();
});

it.each([31, 0, 1, 127])("rejects raw callback control character %i without adding a pending request", (code) => {
  const changed = vi.fn();
  const mock = listening(changed);
  const before = read(mock);
  changed.mockClear();
  const callbackOrigin = "https://assistant.example.test" + String.fromCharCode(code);
  expect(() => mock.addPendingClient("Example assistant", callbackOrigin)).toThrow(/HTTPS callback origin/);
  expect(read(mock)).toEqual(before);
  expect(changed).not.toHaveBeenCalled();
});
