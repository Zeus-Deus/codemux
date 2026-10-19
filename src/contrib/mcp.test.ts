import { beforeEach, describe, expect, it, vi } from "vitest";
import { Owner, ownedDisposer } from "./lifetime";

const mocks = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn(), sync: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));
vi.mock("@/stores/mcp-store", () => ({ useMcpStore: { getState: () => ({ syncToBackend: mocks.sync }) } }));
import { createMcpHost, registerMcpServer, type PluginMcpServer } from "./mcp";

const server: PluginMcpServer = { id: "editor", name: "Editor", toolPrefix: "unity", scope: { workspaceIds: ["project-a"] }, transport: "stdio", command: "unity", args: ["mcp"], cwd: "/tmp/project-a" };
const flush = async () => { await Promise.resolve(); await Promise.resolve(); await Promise.resolve(); };
function api(owner: Owner) {
  return createMcpHost(() => owner, (fn, attach) => ownedDisposer(owner, attach(fn)));
}
beforeEach(() => {
  vi.clearAllMocks();
  mocks.sync.mockResolvedValue(undefined);
  mocks.invoke.mockResolvedValue(undefined);
  mocks.listen.mockResolvedValue(() => {});
});
describe("plugin MCP", () => {
  it("registers the scoped config, syncs stable disable choices first, and disposes once", async () => {
    const owner = new Owner("unity");
    const dispose = await registerMcpServer(owner, server);
    expect(mocks.sync).toHaveBeenCalledOnce();
    expect(mocks.sync.mock.invocationCallOrder[0]).toBeLessThan(mocks.invoke.mock.invocationCallOrder[0]);
    expect(mocks.invoke).toHaveBeenCalledWith("plugins_mcp_register", expect.objectContaining({ pluginId: "unity", server }));
    dispose(); owner.end();
    expect(mocks.invoke.mock.calls.filter(([name]) => name === "plugins_mcp_unregister")).toHaveLength(1);
  });
  it("tears down when disabled before async registration settles", async () => {
    let finish!: () => void;
    mocks.invoke.mockImplementation((name: string) => name === "plugins_mcp_register" ? new Promise<void>((resolve) => { finish = resolve; }) : Promise.resolve());
    const owner = new Owner("unity");
    const pending = registerMcpServer(owner, server);
    await flush(); owner.end(); finish(); await pending;
    expect(mocks.invoke).toHaveBeenCalledWith("plugins_mcp_unregister", expect.objectContaining({ pluginId: "unity", localId: "editor" }));
    expect(() => api(owner).getStatus("editor")).toThrow("no longer active");
  });
  it("a late disposer cannot address a replacement registration", async () => {
    const owner = new Owner("unity");
    const first = await registerMcpServer(owner, server);
    await registerMcpServer(owner, server);
    const tokens = mocks.invoke.mock.calls.filter(([name]) => name === "plugins_mcp_register").map(([, args]) => args.token);
    first();
    await api(owner).getStatus("editor");
    expect(tokens[0]).not.toBe(tokens[1]);
    expect(mocks.invoke).toHaveBeenLastCalledWith("plugins_mcp_status", { pluginId: "unity", localId: "editor", token: tokens[1] });
    owner.end();
  });
  it("preserves image content and structured results outside a chat", async () => {
    const owner = new Owner("unity");
    await registerMcpServer(owner, server);
    const result = { content: [{ type: "image", mimeType: "image/png", data: "base64-intact" }], structuredContent: { ok: true } };
    mocks.invoke.mockResolvedValue(result);
    expect(await api(owner).callTool("editor", "screenshot", {})).toBe(result);
    expect(() => api(new Owner("other")).listTools("editor")).toThrow("no longer active");
    owner.end();
  });
  it("filters activity by owned server and removes a subscription attached after unload", async () => {
    const owner = new Owner("unity");
    await registerMcpServer(owner, server);
    const callback = vi.fn();
    const unlisten = vi.fn();
    let attach!: (fn: () => void) => void;
    mocks.listen.mockReturnValue(new Promise<() => void>((resolve) => { attach = resolve; }));
    api(owner).onActivity("editor", callback);
    const listener = mocks.listen.mock.calls[0][1];
    listener({ payload: { serverId: "plugin:other:editor", phase: "start" } });
    expect(callback).not.toHaveBeenCalled();
    listener({ payload: { serverId: "plugin:unity:editor", phase: "start", callId: "one" } });
    expect(callback).toHaveBeenCalledOnce();
    owner.end(); attach(unlisten); await flush();
    expect(unlisten).toHaveBeenCalledOnce();
    listener({ payload: { serverId: "plugin:unity:editor", phase: "end", callId: "one" } });
    expect(callback).toHaveBeenCalledOnce();
  });
});
