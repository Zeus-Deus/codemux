import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { useCustomAcpThread } from "./use-custom-acp-thread";
import { useCustomAcp } from "@/stores/custom-acp-store";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { agentChatStartSession, agentChatStopSession } from "@/tauri/commands";
import type { AcpCatalog, AcpThreadCatalog } from "@/tauri/custom-acp";
import { useAgentChatEvents } from "./use-agent-chat-events";
import type { AgentChatEventPayload } from "@/tauri/events";
const { invoke, listen, listeners, channels } = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn(), listeners: new Map<string, (event: { payload: unknown }) => void>(), channels: [] as { onmessage: (payload: AgentChatEventPayload) => void }[] }));
vi.mock("@tauri-apps/api/core", () => ({ invoke, Channel: class { constructor(public onmessage: (payload: AgentChatEventPayload) => void) { channels.push(this); } } }));
vi.mock("@tauri-apps/api/event", () => ({ listen, emit: vi.fn() }));
const agent = { id: "id", name: "A", executable: "cli", args: [], environment: {}, enabled: true, auth_method: null, revision: "r" };
const catalog: AcpCatalog = { agent_id: agent.id, agent_name: agent.name, capabilities: { models: [], effort_granularity: "per_session", effort_label_map: {}, permission_modes: [], default_permission_mode: null, permission_granularity: "per_session" }, config_options: [], current_model: null, supports_resume: false, supports_images: false, auth_methods: [] };
const binding = { thread_id: "bound", agent_id: agent.id, revision: agent.revision, cwd: "/repo", session_id: "s", catalog, config_values: {} };
beforeEach(() => {
  useCustomAcp.setState({ agents: null, selections: {}, bindings: {}, catalogs: {}, threadCatalogs: {}, restored: {}, errors: {}, busy: {}, live: {} });
  invoke.mockReset(); listen.mockReset(); listeners.clear(); channels.length = 0;
  listen.mockImplementation(async (name: string, cb: (event: { payload: unknown }) => void) => { listeners.set(name, cb); return () => listeners.delete(name); });
  invoke.mockImplementation(async (cmd: string) => cmd === "acp_agents" ? [agent] : cmd === "acp_binding" ? binding : { catalog, live: false });
});
afterEach(cleanup);
it("R3-final-1: cold read admitted during post-start metadata still owns later status", async () => {
  let started = false;
  let resolveBinding!: (value: typeof binding) => void;
  let resolveCatalog!: (value: AcpThreadCatalog) => void;
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === "acp_agents") return [{ ...agent, revision: "changed" }];
    if (cmd === "acp_binding") return started ? new Promise<typeof binding>(r => { resolveBinding = r; }) : binding;
    if (cmd === "acp_thread_catalog") return started ? new Promise<AcpThreadCatalog>(r => { resolveCatalog = r; }) : { catalog, live: true };
    if (cmd === "agent_chat_start_session") { started = true; return "bound"; }
    throw new Error(`Unexpected command: ${cmd}`);
  });
  const { result } = renderHook(() => useCustomAcpThread("bound", true));
  await waitFor(() => expect(result.current.ready).toBe(true));
  const starting = agentChatStartSession("pane", "acp", { thread_id: "bound", cwd: "/repo", model: null, resume_cursor: null, permission_mode: null, additional_directories: [], env: null });
  await waitFor(() => expect(resolveBinding).toBeTypeOf("function"));
  const reading = useCustomAcp.getState().refreshThread("bound");
  await act(async () => { resolveBinding(binding); await starting; });
  await act(async () => { resolveCatalog({ catalog, live: false }); await reading; });
  expect(useCustomAcp.getState().live.bound).toBe(false);
  expect(result.current.ready).toBe(false);
  expect(result.current.error).toMatch(/launch configuration changed/);
});
it("confirms a delayed verified start without newer lifecycle evidence and then trusts current native readback", async () => {
  let started = false;
  let nativeLive = false;
  let resolveBinding!: (value: typeof binding) => void;
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === "acp_binding") return started ? new Promise<typeof binding>(r => { resolveBinding = r; }) : binding;
    if (cmd === "agent_chat_start_session") { started = true; nativeLive = true; return "bound"; }
    if (cmd === "acp_thread_catalog") return { catalog, live: nativeLive };
    throw new Error(`Unexpected command: ${cmd}`);
  });
  const starting = agentChatStartSession("pane", "acp", { thread_id: "bound", cwd: "/repo", model: null, resume_cursor: null, permission_mode: null, additional_directories: [], env: null });
  await waitFor(() => expect(resolveBinding).toBeTypeOf("function"));
  expect(useCustomAcp.getState().live.bound).toBeUndefined();
  resolveBinding(binding);
  await expect(starting).resolves.toBe("bound");
  expect(useCustomAcp.getState().live.bound).toBe(true);
  nativeLive = false;
  await useCustomAcp.getState().refreshThread("bound");
  expect(useCustomAcp.getState().live.bound).toBe(false);
});
it("R3-final-2: delayed start metadata cannot reset a newer exact-owner catalog", async () => {
  let started = false;
  let current = catalog;
  let resolveBinding!: (value: typeof binding) => void;
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === "acp_agents") return [agent];
    if (cmd === "acp_binding") return started ? new Promise<typeof binding>(r => { resolveBinding = r; }) : binding;
    if (cmd === "acp_thread_catalog") return { catalog: current, live: true };
    if (cmd === "agent_chat_start_session") { started = true; return "bound"; }
    throw new Error(`Unexpected command: ${cmd}`);
  });
  const { result } = renderHook(() => useCustomAcpThread("bound", true));
  await waitFor(() => expect(result.current.ready).toBe(true));
  const starting = agentChatStartSession("pane", "acp", { thread_id: "bound", cwd: "/repo", model: null, resume_cursor: null, permission_mode: null, additional_directories: [], env: null });
  await waitFor(() => expect(resolveBinding).toBeTypeOf("function"));
  current = { ...catalog, current_model: "new/current", supports_images: true };
  await act(async () => useCustomAcp.getState().refreshThread("bound"));
  expect(result.current.catalog).toEqual(current);
  await act(async () => { resolveBinding(binding); await starting; });
  expect(result.current.catalog).toEqual(current);
  expect(useAgentChatStore.getState().threads.bound.model).toBe(current.current_model);
  expect(useCustomAcp.getState().bindings.bound?.session_id).toBe(binding.session_id);
});
it.each(["ready", "running", "start"] as const)("R3-final-2: sole fresh reload catalog survives matching %s evidence", async evidence => {
  const current: AcpCatalog = { ...catalog, current_model: " opaque/Current ", supports_images: true, config_options: [{ id: " replacement /thought ", category: "thought_level", name: "Fresh thought level", description: null, type: "select", current_value: "ultrathink", options: [{ value: "ultrathink", name: "Opaque high", description: null }] }] };
  let resolveCatalog!: (value: AcpThreadCatalog) => void;
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === "acp_agents") return [{ ...agent, revision: "changed" }];
    if (cmd === "acp_binding") return binding;
    if (cmd === "acp_thread_catalog") return new Promise<AcpThreadCatalog>(r => { resolveCatalog = r; });
    if (cmd === "agent_chat_start_session") return "bound";
    if (cmd === "attach_agent_chat_output") return 1;
    if (cmd === "detach_agent_chat_output") return;
    throw new Error(`Unexpected command: ${cmd}`);
  });
  const { result } = renderHook(() => {
    useAgentChatEvents("bound", () => {});
    return useCustomAcpThread("bound", true);
  });
  await waitFor(() => expect(resolveCatalog).toBeTypeOf("function"));
  expect(result.current.catalog).toEqual(catalog);
  if (evidence === "start") await act(async () => agentChatStartSession("pane", "acp", { thread_id: "bound", cwd: "/repo", model: null, resume_cursor: null, permission_mode: null, additional_directories: [], env: null }));
  else act(() => channels[0].onmessage({ thread_id: "bound", event: { type: "session_state_changed", thread_id: "bound", status: evidence === "ready" ? { status: "ready" } : { status: "running", active_turn: "turn" } } }));
  await act(async () => resolveCatalog({ catalog: current, live: false }));
  await waitFor(() => expect(result.current.loading).toBe(false));
  expect(result.current.catalog).toEqual(current);
  expect(useAgentChatStore.getState().threads.bound.model).toBe(current.current_model);
  expect(useCustomAcp.getState().live.bound).toBe(true);
  expect(result.current.ready).toBe(true);
  expect(result.current.warning).toMatch(/launch configuration changed/);
  expect(invoke.mock.calls.filter(([cmd]) => cmd === "acp_thread_catalog")).toHaveLength(1);
});
it.each(["closed", "error", "Stop", "cold read"] as const)("R3-final-1: delayed post-start binding cannot resurrect newer %s evidence", async evidence => {
  let started = false;
  let nativeLive = true;
  let resolveBinding!: (value: typeof binding) => void;
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === "acp_agents") return [{ ...agent, revision: "changed" }];
    if (cmd === "acp_binding") return started ? new Promise<typeof binding>(r => { resolveBinding = r; }) : binding;
    if (cmd === "acp_thread_catalog") return { catalog, live: nativeLive };
    if (cmd === "agent_chat_start_session") { started = true; return "bound"; }
    if (cmd === "agent_chat_stop_session" || cmd === "detach_agent_chat_output") return;
    if (cmd === "attach_agent_chat_output") return 1;
    throw new Error(`Unexpected command: ${cmd}`);
  });
  const { result } = renderHook(() => {
    useAgentChatEvents("bound", () => {});
    return useCustomAcpThread("bound", true);
  });
  await waitFor(() => expect(result.current.ready).toBe(true));
  expect(result.current.warning).toMatch(/launch configuration changed/);
  const starting = agentChatStartSession("pane", "acp", { thread_id: "bound", cwd: "/repo", model: null, resume_cursor: null, permission_mode: null, additional_directories: [], env: null });
  await waitFor(() => expect(resolveBinding).toBeTypeOf("function"));
  nativeLive = false;
  if (evidence === "Stop") await act(async () => agentChatStopSession("acp", "bound"));
  else if (evidence === "cold read") await act(async () => useCustomAcp.getState().refreshThread("bound"));
  else act(() => channels[0].onmessage({ thread_id: "bound", event: { type: "session_state_changed", thread_id: "bound", status: evidence === "closed" ? { status: "closed" } : { status: "error", message: "process lost" } } }));
  expect(useCustomAcp.getState().live.bound).toBe(false);
  await act(async () => { resolveBinding(binding); await starting; });
  expect(invoke.mock.calls.filter(([cmd]) => cmd === "acp_binding")).toHaveLength(3);
  expect(useCustomAcp.getState().bindings.bound).toEqual(binding);
  expect(useCustomAcp.getState().live.bound).toBe(false);
  expect(result.current.ready).toBe(false);
  expect(result.current.error).toMatch(/launch configuration changed/);
});
it.each([
  { label: "revised", agents: [{ ...agent, revision: "changed" }], reason: /changed/ },
  { label: "disabled", agents: [{ ...agent, enabled: false }], reason: /disabled/ },
  { label: "deleted", agents: [], reason: /removed/ },
])("keeps a cold $label binding denied despite a cached catalog", async ({ agents, reason }) => {
  useCustomAcp.setState({ bindings: { bound: binding }, threadCatalogs: { bound: catalog }, live: { bound: true } });
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === "acp_agents") return agents;
    if (cmd === "acp_binding") return binding;
    if (cmd === "acp_thread_catalog") return { catalog, live: false };
    throw new Error(`Unexpected command: ${cmd}`);
  });
  const { result } = renderHook(() => useCustomAcpThread("bound", true));
  await waitFor(() => expect(result.current.loading).toBe(false));
  expect(result.current.ready).toBe(false);
  expect(result.current.error).toMatch(reason);
  expect(useCustomAcp.getState().live.bound).toBe(false);
  expect(invoke.mock.calls.some(([cmd]) => cmd === "agent_chat_start_session")).toBe(false);
});
it("a newer live read supersedes an older cold read without inventing durable liveness", async () => {
  const resolvers: ((value: AcpThreadCatalog) => void)[] = [];
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === "acp_agents") return [{ ...agent, revision: "changed" }];
    if (cmd === "acp_binding") return binding;
    if (cmd === "acp_thread_catalog") return new Promise<AcpThreadCatalog>(r => resolvers.push(r));
    throw new Error(`Unexpected command: ${cmd}`);
  });
  const { result } = renderHook(() => useCustomAcpThread("bound", true));
  await waitFor(() => expect(resolvers).toHaveLength(1));
  act(() => listeners.get("custom_acp_catalog_changed")!({ payload: { thread_id: "bound", catalog } }));
  await waitFor(() => expect(resolvers).toHaveLength(2));
  const current = { ...catalog, current_model: "new/current" };
  await act(async () => resolvers[1]({ catalog: current, live: true }));
  await act(async () => resolvers[0]({ catalog, live: false }));
  await waitFor(() => expect(result.current.ready).toBe(true));
  expect(result.current.catalog).toEqual(current);
  expect(useCustomAcp.getState().live.bound).toBe(true);
  const persisted = JSON.parse(localStorage.getItem("codemux:custom-acp:v1")!);
  expect(Object.keys(persisted.state)).toEqual(["selections"]);
});
it.each(["closed", "error"] as const)("does not let a late live read reopen a session after native %s", async status => {
  let resolve!: (value: AcpThreadCatalog) => void;
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === "acp_agents") return [{ ...agent, revision: "changed" }];
    if (cmd === "acp_binding") return binding;
    if (cmd === "acp_thread_catalog") return new Promise<AcpThreadCatalog>(r => { resolve = r; });
    if (cmd === "attach_agent_chat_output") return 1;
    if (cmd === "detach_agent_chat_output") return;
    throw new Error(`Unexpected command: ${cmd}`);
  });
  const { result } = renderHook(() => {
    useAgentChatEvents("bound", () => {});
    return useCustomAcpThread("bound", true);
  });
  await waitFor(() => expect(resolve).toBeTypeOf("function"));
  act(() => channels[0].onmessage({ thread_id: "bound", event: { type: "session_state_changed", thread_id: "bound", status: status === "closed" ? { status: "closed" } : { status: "error", message: "process lost" } } }));
  expect(useCustomAcp.getState().live.bound).toBe(false);
  const currentCatalog = { ...catalog, current_model: "last/native" };
  await act(async () => resolve({ catalog: currentCatalog, live: true }));
  await waitFor(() => expect(result.current.loading).toBe(false));
  expect(useCustomAcp.getState().live.bound).toBe(false);
  expect(result.current.ready).toBe(false);
  expect(result.current.error).toMatch(/launch configuration changed/);
  // Closed/error is newer status, not a replacement model/config catalog.
  expect(result.current.catalog).toEqual(currentCatalog);
  expect(useAgentChatStore.getState().threads.bound.model).toBe(currentCatalog.current_model);
});
it("recovers idle native liveness after a full renderer reload with a revised definition and no lifecycle replay", async () => {
  const current = { ...catalog, current_model: " opaque/live " };
  invoke.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
    if (cmd === "acp_agents") return [{ ...agent, revision: "changed" }];
    if (cmd === "acp_binding") return binding;
    if (cmd === "acp_catalog" || cmd === "acp_thread_catalog") {
      expect(args).toEqual({ threadId: binding.thread_id });
      return cmd === "acp_catalog" ? current : { catalog: current, live: true };
    }
    throw new Error(`Unexpected command: ${cmd}`);
  });
  expect(useCustomAcp.getState().live).toEqual({});
  const { result } = renderHook(() => useCustomAcpThread(binding.thread_id, true));
  await waitFor(() => expect(result.current.loading).toBe(false));
  expect(result.current.ready).toBe(true);
  expect(result.current.error).toBeNull();
  expect(result.current.warning).toMatch(/launch configuration changed/);
  expect(result.current.catalog).toEqual(current);
  expect(useCustomAcp.getState().live.bound).toBe(true);
  expect(invoke).toHaveBeenCalledWith("acp_thread_catalog", { threadId: binding.thread_id });
  expect(invoke.mock.calls.some(([cmd]) => cmd === "agent_chat_start_session")).toBe(false);
});
it("subscribes through native events and reads definitions after external changes", async () => {
  const { result, unmount } = renderHook(() => useCustomAcpThread("bound", true));
  await waitFor(() => expect(result.current.ready).toBe(true));
  expect(listeners.has("custom_acp_changed")).toBe(true);
  invoke.mockImplementation(async (cmd: string) => cmd === "acp_agents" ? [{ ...agent, enabled: false }] : cmd === "acp_binding" ? binding : { catalog, live: false });
  act(() => listeners.get("custom_acp_changed")!({ payload: null }));
  await waitFor(() => expect(result.current.error).toMatch(/disabled/));
  unmount();
  await waitFor(() => expect(listeners.size).toBe(0));
});
it("refreshes authoritative live controls after native catalog events while idle", async () => {
  const { result } = renderHook(() => useCustomAcpThread("bound", true));
  await waitFor(() => expect(result.current.ready).toBe(true));
  expect(listeners.has("custom_acp_catalog_changed")).toBe(true);
  const updated = { ...catalog, current_model: " Opaque/ID ", supports_images: true };
  invoke.mockImplementation(async (cmd: string) => cmd === "acp_thread_catalog" ? { catalog: updated, live: true } : cmd === "acp_agents" ? [agent] : binding);
  act(() => listeners.get("custom_acp_catalog_changed")!({ payload: { thread_id: "bound", catalog: updated } }));
  await waitFor(() => expect(result.current.catalog?.current_model).toBe(" Opaque/ID "));
  expect(result.current.catalog?.supports_images).toBe(true);
});

it("reads live catalog before changed-revision launch checks, survives remount, and blocks cold restart", async () => {
  let nativeLive = false;
  let revision = agent.revision;
  let liveCatalog = catalog;
  // Transport contract: a matching live instance owns catalog reads; only
  // a cold read/start consults the current saved definition's revision.
  invoke.mockImplementation(async (cmd: string, args?: Record<string, any>) => {
    switch (cmd) {
      case "acp_agents": return [{ ...agent, revision }];
      case "acp_binding": return binding;
      case "acp_thread_catalog":
        expect(args).toEqual({ threadId: "bound" });
        if (nativeLive) return { catalog: liveCatalog, live: true };
        if (revision !== binding.revision) throw new Error("launch configuration changed");
        return { catalog: binding.catalog, live: false };
      case "agent_chat_start_session":
        if (revision !== binding.revision) throw new Error("launch configuration changed");
        nativeLive = true; return "bound";
      case "agent_chat_stop_session": nativeLive = false; return;
      default: throw new Error(`Unexpected command: ${cmd}`);
    }
  });
  await agentChatStartSession("pane", "acp", { thread_id: "bound", cwd: "/repo", model: null, resume_cursor: null, permission_mode: null, additional_directories: [], env: null });
  expect(useCustomAcp.getState().live.bound).toBe(true);
  let view = renderHook(() => useCustomAcpThread("bound", true));
  await waitFor(() => expect(view.result.current.ready).toBe(true));
  revision = "changed";
  act(() => listeners.get("custom_acp_changed")!({ payload: null }));
  await waitFor(() => expect(view.result.current.warning).toMatch(/launch configuration changed/));
  liveCatalog = { ...catalog, current_model: " exact/live ", supports_images: true };
  act(() => listeners.get("custom_acp_catalog_changed")!({ payload: { thread_id: "bound", catalog: { ...catalog, current_model: "not-read-authority" } } }));
  await waitFor(() => expect(view.result.current.catalog?.current_model).toBe(" exact/live "));
  expect(view.result.current.ready).toBe(true);
  view.unmount();
  view = renderHook(() => useCustomAcpThread("bound", true));
  await waitFor(() => expect(view.result.current.ready).toBe(true));
  expect(view.result.current.warning).toMatch(/launch configuration changed/);
  expect(view.result.current.catalog?.current_model).toBe(" exact/live ");
  view.unmount();
  await agentChatStopSession("acp", "bound");
  expect(useCustomAcp.getState().live.bound).toBe(false);
  view = renderHook(() => useCustomAcpThread("bound", true));
  await waitFor(() => expect(view.result.current.error).toMatch(/launch configuration changed/));
  expect(view.result.current.ready).toBe(false);
  await expect(agentChatStartSession("pane", "acp", { thread_id: "bound", cwd: "/repo", model: null, resume_cursor: { resume: "s" }, permission_mode: null, additional_directories: [], env: null })).rejects.toThrow("launch configuration changed");
  expect(useCustomAcp.getState().live.bound).toBe(false);
});
it("does not suppress an unknown catalog read failure just because a changed-revision session is live", async () => {
  useCustomAcp.setState({ live: { bound: true } });
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === "acp_agents") return [{ ...agent, revision: "changed" }];
    if (cmd === "acp_binding") return binding;
    if (cmd === "acp_thread_catalog") throw new Error("catalog storage unavailable");
    throw new Error(`Unexpected command: ${cmd}`);
  });
  const { result } = renderHook(() => useCustomAcpThread("bound", true));
  await waitFor(() => expect(result.current.error).toMatch(/catalog storage unavailable/));
  expect(result.current.ready).toBe(false);
  expect(result.current.loading).toBe(false);
});
