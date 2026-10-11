import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ComponentProps } from "react";
import { useAgentChatEvents } from "@/hooks/use-agent-chat-events";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import type { AgentChatEventPayload } from "@/tauri/events";
import { Composer } from "./Composer";
import { TooltipProvider } from "@/components/ui/tooltip";
import { useCustomAcp, acpCatalogKey } from "@/stores/custom-acp-store";
import { MultiProviderModelPicker } from "./pickers/MultiProviderModelPicker";
import { CustomAcpModels } from "./pickers/CustomAcpModels";
import { agentChatStartSession, agentChatStopSession } from "@/tauri/commands";
import type { AcpThreadCatalog } from "@/tauri/custom-acp";
import { useAppStore } from "@/stores/app-store";
const { invoke, channels, listeners } = vi.hoisted(() => ({ invoke: vi.fn(), channels: [] as { onmessage: (payload: AgentChatEventPayload) => void }[], listeners: new Map<string, (event: { payload: unknown }) => void>() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke, Channel: class { constructor(public onmessage: (payload: AgentChatEventPayload) => void) { channels.push(this); } } }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async (name: string, callback: (event: { payload: unknown }) => void) => { listeners.set(name, callback); return () => listeners.delete(name); }), emit: vi.fn() }));
const agent = { id: "stable-agent", name: "Local", executable: "cli", args: [], environment: {}, enabled: true, auth_method: null, revision: "r" };
const catalog = { agent_id: agent.id, agent_name: agent.name, capabilities: { models: [], effort_granularity: "per_session" as const, effort_label_map: {}, permission_modes: [], default_permission_mode: null, permission_granularity: "per_session" as const }, config_options: [], current_model: null, supports_images: false, supports_resume: true, auth_methods: [] };
const props = (): ComponentProps<typeof Composer> => ({ threadId: "draft-thread", draft: "hello", cwd: "/repo", provider: "acp", model: null, permissionMode: null, effort: null, contextWindow: null, activeModel: null, effortLabelMap: {}, permissionModes: null, ultrathinkInBodyText: false, streaming: false, sessionReady: true, showProviderPicker: true, mode: "default", onDraftChange: vi.fn(), onSubmit: vi.fn(), onStop: vi.fn(), onProviderModelChange: vi.fn(), onModelChange: vi.fn(), onPermissionModeChange: vi.fn(), onEffortChange: vi.fn(), onContextWindowChange: vi.fn(), onModeActivate: vi.fn(), onModeRemove: vi.fn() });
beforeEach(() => {
  useAppStore.setState({ appState: null });
  useCustomAcp.setState({ agents: [agent], selections: { "draft-thread": agent.id }, bindings: {}, catalogs: { [acpCatalogKey(agent.id, agent.revision)]: catalog }, threadCatalogs: {}, restored: {}, reads: {}, errors: {}, busy: {}, live: {} });
  invoke.mockReset(); channels.length = 0; listeners.clear();
  invoke.mockImplementation(async (cmd: string) => cmd === "acp_agents" ? [agent] : cmd === "acp_binding" ? null : cmd === "agent_chat_provider_health" ? { status: "ready", installed: true, message: null, version: null } : cmd === "list_chat_provider_capabilities" ? catalog.capabilities : []);
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });
it("F3: successful manual catalog refresh recovers every mounted consumer after initial failure", async () => {
  const binding = { thread_id: "draft-thread", agent_id: agent.id, revision: agent.revision, cwd: "/repo", session_id: "raw", catalog, config_values: {} };
  let available = false;
  let resolveRead!: (value: AcpThreadCatalog) => void;
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === "acp_agents") return [agent];
    if (cmd === "acp_binding") return binding;
    if (cmd === "acp_thread_catalog") {
      if (!available) throw new Error("catalog read unavailable");
      return new Promise<AcpThreadCatalog>(r => { resolveRead = r; });
    }
    return [];
  });
  const p = props();
  const { container } = render(<TooltipProvider><Composer {...p} /><CustomAcpModels threadId={p.threadId} model={null} onSelect={vi.fn()} /></TooltipProvider>);
  const textarea = container.querySelector("textarea")!;
  await waitFor(() => expect(screen.getAllByText(/catalog read unavailable/)).toHaveLength(2));
  expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
  available = true;
  fireEvent.click(screen.getByRole("button", { name: "Refresh session controls" }));
  await waitFor(() => expect(resolveRead).toBeTypeOf("function"));
  expect(screen.getAllByText(/catalog read unavailable/)).toHaveLength(2);
  fireEvent.keyDown(textarea, { key: "Enter" });
  expect(p.onSubmit).not.toHaveBeenCalled();
  await act(async () => resolveRead({ catalog, live: false }));
  await waitFor(() => expect(screen.getByRole("button", { name: "Send" })).not.toBeDisabled());
  expect(screen.queryByText(/catalog read unavailable/)).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Refresh session controls" })).not.toBeDisabled();
  fireEvent.keyDown(textarea, { key: "Enter" });
  expect(p.onSubmit).toHaveBeenCalledOnce();
  expect(useCustomAcp.getState().live["draft-thread"]).toBe(false);
  expect(container.querySelector("textarea")).toBe(textarea);
});
it.each(["enabled", "revision"] as const)("F3: retries failed initial reads after an authoritative definition %s update", async change => {
  const binding = { thread_id: "draft-thread", agent_id: agent.id, revision: agent.revision, cwd: "/repo", session_id: "raw", catalog, config_values: {} };
  let changed = false;
  let resolveRead!: (value: AcpThreadCatalog) => void;
  let reads = 0;
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === "acp_agents") return [{ ...agent, enabled: change === "enabled" ? changed : true, revision: change === "revision" && !changed ? "edited" : agent.revision }];
    if (cmd === "acp_binding") return binding;
    if (cmd === "acp_thread_catalog") {
      reads++;
      if (!changed) throw new Error("saved agent unavailable");
      return new Promise<AcpThreadCatalog>(r => { resolveRead = r; });
    }
    return [];
  });
  const p = props();
  render(<TooltipProvider><Composer {...p} /><CustomAcpModels threadId={p.threadId} model={null} onSelect={vi.fn()} /></TooltipProvider>);
  await waitFor(() => expect(screen.getAllByText(/saved agent unavailable/)).toHaveLength(2));
  const initialReads = reads;
  changed = true;
  await act(async () => useCustomAcp.getState().loadAgents(true));
  await waitFor(() => expect(resolveRead).toBeTypeOf("function"));
  expect(reads).toBe(initialReads + 1);
  expect(screen.getAllByText(/saved agent unavailable/)).toHaveLength(2);
  expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
  await act(async () => resolveRead({ catalog, live: false }));
  await waitFor(() => expect(screen.getByRole("button", { name: "Send" })).not.toBeDisabled());
  expect(screen.queryByText(/saved agent unavailable/)).not.toBeInTheDocument();
  await act(async () => useCustomAcp.getState().loadAgents(true));
  expect(reads).toBe(initialReads + 1);
});
it.each((["enabled", "revision"] as const).flatMap(change => [false, true].map(retryFails => ({ change, retryFails }))))
("P2: repair $change before initial rejection reaches both consumers with one bounded retry (retryFails=$retryFails)", async ({ change, retryFails }) => {
  const binding = { thread_id: "draft-thread", agent_id: agent.id, revision: agent.revision, cwd: "/repo", session_id: "raw", catalog, config_values: {} };
  const pending: { resolve: (value: AcpThreadCatalog) => void; reject: (reason: Error) => void }[] = [];
  let repaired = false;
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === "acp_agents") return [{ ...agent, enabled: change === "enabled" ? repaired : true, revision: change === "revision" && !repaired ? "old-definition" : agent.revision }];
    if (cmd === "acp_binding") return binding;
    if (cmd === "acp_thread_catalog") return new Promise<AcpThreadCatalog>((resolve, reject) => pending.push({ resolve, reject }));
    return [];
  });
  const p = props();
  const content = <TooltipProvider><Composer {...p} /><CustomAcpModels threadId={p.threadId} model={null} onSelect={vi.fn()} /></TooltipProvider>;
  const view = render(content);
  const textarea = view.container.querySelector("textarea")!;
  try {
    await waitFor(() => expect(pending).toHaveLength(1));
    expect(useCustomAcp.getState().reads["draft-thread"].error).toBeNull();
    repaired = true;
    await act(async () => useCustomAcp.getState().loadAgents(true));
    expect(pending).toHaveLength(1);
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    await act(async () => pending[0].reject(new Error("initial catalog before repair failed")));
    expect(pending).toHaveLength(2);
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    expect(screen.getAllByText(/initial catalog before repair failed/)).toHaveLength(2);
    fireEvent.keyDown(textarea, { key: "Enter" });
    fireEvent.keyDown(textarea, { key: "Enter", ctrlKey: true });
    expect(p.onSubmit).not.toHaveBeenCalled();
    await act(async () => useCustomAcp.getState().loadAgents(true));
    expect(pending).toHaveLength(2);
    if (retryFails) {
      await act(async () => pending[1].reject(new Error("repair retry failed")));
      expect(screen.getAllByText(/repair retry failed/)).toHaveLength(2);
      expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
      await act(async () => { await useCustomAcp.getState().loadAgents(true); await useCustomAcp.getState().loadAgents(true); });
      expect(pending).toHaveLength(2);
      const refresh = screen.getByRole("button", { name: "Refresh session controls" });
      expect(refresh).not.toBeDisabled();
      fireEvent.click(refresh);
      await act(async () => pending[2].resolve({ catalog, live: false }));
    } else await act(async () => pending[1].resolve({ catalog, live: false }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Send" })).not.toBeDisabled());
    expect(screen.queryByText(/catalog before repair failed|repair retry failed/)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Refresh session controls" })).not.toBeDisabled();
    expect(useCustomAcp.getState().live["draft-thread"]).toBe(false);
    expect(view.container.querySelector("textarea")).toBe(textarea);
    expect(pending).toHaveLength(retryFails ? 3 : 2);
    await act(async () => useCustomAcp.getState().loadAgents(true));
    expect(pending).toHaveLength(retryFails ? 3 : 2);
    expect(invoke.mock.calls.some(([cmd]) => cmd === "agent_chat_start_session" || cmd === "acp_probe")).toBe(false);
  } finally {
    await act(async () => { for (const read of pending) read.resolve({ catalog, live: false }); });
  }
});

it.each((["enabled", "revision"] as const).flatMap(change => [false, true].map(retryFails => ({ change, retryFails }))))
("FE2 Composer: native repair after catalog handoff stays bounded ($change, retryFails=$retryFails)", async ({ change, retryFails }) => {
  const binding = { thread_id: "draft-thread", agent_id: agent.id, revision: agent.revision, cwd: "/repo", session_id: "raw", catalog, config_values: {} };
  const pending: { resolve: (value: AcpThreadCatalog) => void; reject: (reason: Error) => void }[] = [];
  let repaired = false;
  invoke.mockImplementation(async cmd => {
    if (cmd === "acp_agents") return [{ ...agent, enabled: change === "enabled" ? repaired : true, revision: change === "revision" && !repaired ? "before-repair" : agent.revision }];
    if (cmd === "acp_binding") return binding;
    if (cmd === "acp_thread_catalog") return new Promise<AcpThreadCatalog>((resolve, reject) => pending.push({ resolve, reject }));
    return [];
  });
  const p = props();
  const view = render(<TooltipProvider><Composer {...p} /><CustomAcpModels threadId={p.threadId} model={null} onSelect={vi.fn()} /></TooltipProvider>);
  const textarea = view.container.querySelector("textarea")!;
  try {
    await waitFor(() => expect(pending).toHaveLength(1));
    act(() => listeners.get("custom_acp_catalog_changed")!({ payload: { thread_id: p.threadId } }));
    expect(pending).toHaveLength(2);
    repaired = true;
    act(() => listeners.get("custom_acp_changed")!({ payload: null }));
    await waitFor(() => expect(useCustomAcp.getState().agents?.[0]).toMatchObject(agent));
    expect(useCustomAcp.getState().reads["draft-thread"].error).toBeNull();
    await act(async () => pending[0].reject(new Error("obsolete initial failure")));
    expect(pending).toHaveLength(2);
    await act(async () => pending[1].reject(new Error("latest before repair failed")));
    expect(pending).toHaveLength(3);
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    expect(screen.getAllByText(/^Error: latest before repair failed$/)).toHaveLength(2);
    expect(screen.getAllByText(/Custom agent updates unavailable: Error: latest before repair failed/)).toHaveLength(1);
    act(() => { fireEvent.keyDown(textarea, { key: "Enter" }); fireEvent.keyDown(textarea, { key: "Enter", ctrlKey: true }); });
    expect(p.onSubmit).not.toHaveBeenCalled();
    if (retryFails) await act(async () => pending[2].reject(new Error("one repair retry failed")));
    else await act(async () => pending[2].resolve({ catalog, live: false }));
    expect(screen.getByRole("button", { name: "Send" }).hasAttribute("disabled")).toBe(retryFails);
    if (retryFails) expect(screen.getAllByText(/one repair retry failed/)).toHaveLength(2);
    expect(screen.getAllByText(/Custom agent updates unavailable: Error: latest before repair failed/)).toHaveLength(retryFails ? 1 : 2);
    await act(async () => { await useCustomAcp.getState().loadAgents(true); await useCustomAcp.getState().loadAgents(true); });
    expect(pending).toHaveLength(3);
    expect(view.container.querySelector("textarea")).toBe(textarea);
    expect(textarea.value).toBe("hello");
    expect(invoke.mock.calls.filter(([cmd]) => cmd === "acp_binding")).toHaveLength(1);
    expect(invoke.mock.calls.some(([cmd]) => cmd === "agent_chat_start_session" || cmd === "acp_probe")).toBe(false);
  } finally { await act(async () => { for (const read of pending) read.resolve({ catalog, live: false }); }); }
});

it.each([false, true])("FE3 Composer: latest listing completes delayed binding before submit (bound=%s)", async bound => {
  const binding = { thread_id: "draft-thread", agent_id: agent.id, revision: agent.revision, cwd: "/repo", session_id: "raw", catalog, config_values: {} };
  let rejectList!: (reason: Error) => void;
  let restoreBinding!: (value: typeof binding | null) => void;
  let resolveCatalog!: (value: AcpThreadCatalog) => void;
  let lists = 0;
  invoke.mockImplementation(async cmd => {
    if (cmd === "acp_agents") return ++lists === 1 ? new Promise((_, reject) => { rejectList = reject; }) : [agent];
    if (cmd === "acp_binding") return new Promise<typeof binding | null>(resolve => { restoreBinding = resolve; });
    if (cmd === "acp_thread_catalog") return new Promise<AcpThreadCatalog>(resolve => { resolveCatalog = resolve; });
    if (cmd === "acp_probe") return catalog;
    return [];
  });
  useCustomAcp.setState({ catalogs: {} });
  const p = props();
  const view = render(<TooltipProvider><Composer {...p} /><CustomAcpModels threadId={p.threadId} model={null} onSelect={vi.fn()} /></TooltipProvider>);
  const textarea = view.container.querySelector("textarea")!;
  try {
    await waitFor(() => expect(restoreBinding).toBeTypeOf("function"));
    act(() => listeners.get("custom_acp_changed")!({ payload: null }));
    await waitFor(() => expect(lists).toBe(2));
    await act(async () => {});
    await act(async () => rejectList(new Error("obsolete list failure")));
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    act(() => { fireEvent.keyDown(textarea, { key: "Enter" }); fireEvent.keyDown(textarea, { key: "Enter", ctrlKey: true }); });
    expect(p.onSubmit).not.toHaveBeenCalled();
    await act(async () => restoreBinding(bound ? binding : null));
    if (bound) {
      expect(resolveCatalog).toBeTypeOf("function");
      expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
      await act(async () => resolveCatalog({ catalog, live: false }));
    }
    expect(screen.getByRole("button", { name: "Send" })).not.toBeDisabled();
    expect(screen.queryByText(/obsolete list failure/)).not.toBeInTheDocument();
    if (!bound) {
      fireEvent.click(screen.getByRole("button", { name: "Probe saved agent" }));
      await waitFor(() => expect(invoke).toHaveBeenCalledWith("acp_probe", { agentId: agent.id, cwd: null }));
      expect(screen.getByRole("button", { name: "Send" })).not.toBeDisabled();
    }
    expect(invoke.mock.calls.filter(([cmd]) => cmd === "acp_binding")).toHaveLength(1);
    expect(invoke.mock.calls.filter(([cmd]) => cmd === "acp_thread_catalog")).toHaveLength(bound ? 1 : 0);
    expect(invoke.mock.calls.some(([cmd]) => cmd === "agent_chat_start_session")).toBe(false);
    expect(view.container.querySelector("textarea")).toBe(textarea);
  } finally { await act(async () => { restoreBinding(bound ? binding : null); resolveCatalog?.({ catalog, live: false }); }); }
});

it("F3: deferred null binding and explicit model probe retain the fresh selection's ready sentinel", async () => {
  useCustomAcp.setState({ catalogs: {} });
  let restoreNull!: (value: null) => void;
  let resolveProbe!: (value: typeof catalog) => void;
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === "acp_agents") return [agent];
    if (cmd === "acp_binding") return new Promise<null>(r => { restoreNull = r; });
    if (cmd === "acp_probe") return new Promise<typeof catalog>(r => { resolveProbe = r; });
    return [];
  });
  const p = props();
  const { container } = render(<TooltipProvider><Composer {...p} /><CustomAcpModels threadId={p.threadId} model={null} onSelect={vi.fn()} /></TooltipProvider>);
  const textarea = container.querySelector("textarea")!;
  expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
  await act(async () => restoreNull(null));
  await waitFor(() => expect(screen.getByRole("button", { name: "Send" })).not.toBeDisabled());
  expect(screen.getByRole("button", { name: "Use agent default" })).not.toBeDisabled();
  fireEvent.click(screen.getByRole("button", { name: "Probe saved agent" }));
  await waitFor(() => expect(resolveProbe).toBeTypeOf("function"));
  expect(screen.getByRole("button", { name: "Send" })).not.toBeDisabled();
  await act(async () => resolveProbe(catalog));
  expect(screen.getByRole("button", { name: "Use agent default" })).not.toBeDisabled();
  expect(screen.getByRole("button", { name: "Send" })).not.toBeDisabled();
  expect(container.querySelector("textarea")).toBe(textarea);
  expect(invoke.mock.calls.some(([cmd]) => cmd === "acp_thread_catalog" || cmd === "agent_chat_start_session")).toBe(false);
});
function SubscribedComposer(p: ComponentProps<typeof Composer>) {
  useAgentChatEvents(p.threadId ?? null, () => {});
  return <TooltipProvider><Composer {...p} /></TooltipProvider>;
}
it.each(["closed", "error", "Stop", "cold read"] as const)("R3-final-1 Composer: delayed start metadata preserves denial after %s", async evidence => {
  const binding = { thread_id: "draft-thread", agent_id: agent.id, revision: agent.revision, cwd: "/repo", session_id: "raw", catalog, config_values: {} };
  let started = false;
  let nativeLive = true;
  let resolveBinding!: (value: typeof binding) => void;
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === "acp_agents") return [{ ...agent, revision: "changed" }];
    if (cmd === "acp_binding") return started ? new Promise<typeof binding>(r => { resolveBinding = r; }) : binding;
    if (cmd === "acp_thread_catalog") return { catalog, live: nativeLive };
    if (cmd === "agent_chat_start_session") { started = true; return "draft-thread"; }
    if (cmd === "agent_chat_stop_session" || cmd === "detach_agent_chat_output") return;
    if (cmd === "attach_agent_chat_output") return 1;
    return [];
  });
  const p = props();
  const { container } = render(<SubscribedComposer {...p} />);
  const textarea = container.querySelector("textarea")!;
  await waitFor(() => expect(screen.getByRole("button", { name: "Send" })).not.toBeDisabled());
  const starting = agentChatStartSession("pane", "acp", { thread_id: "draft-thread", cwd: "/repo", model: null, resume_cursor: null, permission_mode: null, additional_directories: [], env: null });
  await waitFor(() => expect(resolveBinding).toBeTypeOf("function"));
  nativeLive = false;
  if (evidence === "Stop") await act(async () => agentChatStopSession("acp", "draft-thread"));
  else if (evidence === "cold read") await act(async () => useCustomAcp.getState().refreshThread("draft-thread"));
  else act(() => channels[0].onmessage({ thread_id: "draft-thread", event: { type: "session_state_changed", thread_id: "draft-thread", status: evidence === "closed" ? { status: "closed" } : { status: "error", message: "process lost" } } }));
  await act(async () => { resolveBinding(binding); await starting; });
  expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
  fireEvent.keyDown(textarea, { key: "Enter" });
  expect(p.onSubmit).not.toHaveBeenCalled();
  expect(container.querySelector("textarea")).toBe(textarea);
  expect(textarea.value).toBe("hello");
  expect(useCustomAcp.getState().live["draft-thread"]).toBe(false);
});
it.each(["ready", "running"] as const)("R3-final-2 Composer: accepted controls readback survives %s without another catalog event", async status => {
  const oldOption = { id: " old /thought ", category: "thought_level", name: "Retired thought level", description: null, type: "select" as const, current_value: "low", options: [{ value: "low", name: "Low", description: null }, { value: "ultrathink", name: "Opaque high", description: null }] };
  const cached = { ...catalog, config_options: [oldOption] };
  const current = { ...catalog, supports_images: true, current_model: " Exact/Model ", config_options: [{ ...oldOption, id: " replacement /thought ", name: "Replacement thought level", current_value: "ultrathink" }] };
  const binding = { thread_id: "draft-thread", agent_id: agent.id, revision: agent.revision, cwd: "/repo", session_id: "raw", catalog: cached, config_values: {} };
  let accepted = false;
  let resolveCatalog!: (value: AcpThreadCatalog) => void;
  let readbackCount = 0;
  invoke.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
    if (cmd === "acp_agents") return [agent];
    if (cmd === "acp_binding") return binding;
    if (cmd === "acp_thread_catalog") {
      if (!accepted) return { catalog: cached, live: true };
      readbackCount++;
      return new Promise<AcpThreadCatalog>(r => { resolveCatalog = r; });
    }
    if (cmd === "acp_set_config") { expect(args).toEqual({ threadId: "draft-thread", configId: oldOption.id, value: "ultrathink" }); accepted = true; return cached; }
    if (cmd === "attach_agent_chat_output") return 1;
    if (cmd === "detach_agent_chat_output") return;
    return [];
  });
  const p = props();
  const view = render(<SubscribedComposer {...p} />);
  await waitFor(() => expect(screen.getByRole("button", { name: "Send" })).not.toBeDisabled());
  fireEvent.click(screen.getByTestId("multi-provider-model-picker-trigger"));
  const retired = await screen.findByLabelText("Retired thought level");
  await waitFor(() => expect(retired).not.toBeDisabled());
  fireEvent.change(retired, { target: { value: "ultrathink" } });
  await waitFor(() => expect(resolveCatalog).toBeTypeOf("function"));
  expect(useCustomAcp.getState().busy["draft-thread"]).toBe(true);
  expect(retired).toBeDisabled();
  act(() => channels[0].onmessage({ thread_id: "draft-thread", event: { type: "session_state_changed", thread_id: "draft-thread", status: status === "ready" ? { status: "ready" } : { status: "running", active_turn: "turn" } } }));
  await act(async () => resolveCatalog({ catalog: current, live: false }));
  const replacement = screen.getByLabelText("Replacement thought level");
  expect(replacement).toHaveValue("ultrathink");
  expect(replacement).not.toBeDisabled();
  expect(screen.queryByLabelText("Retired thought level")).not.toBeInTheDocument();
  expect(useCustomAcp.getState().threadCatalogs["draft-thread"]).toEqual(current);
  expect(useCustomAcp.getState().busy["draft-thread"]).toBe(false);
  expect(useAgentChatStore.getState().threads["draft-thread"].model).toBe(current.current_model);
  expect(readbackCount).toBe(1);
  expect(useCustomAcp.getState().live["draft-thread"]).toBe(true);
  view.rerender(<SubscribedComposer {...p} stagedAttachments={[{ id: "image", kind: "image", ref: "image", metadata: { label: "fixture.png" } }]} />);
  expect(screen.getByRole("button", { name: "Send" })).not.toBeDisabled();
});
it("keeps Composer denied when a pending native live read completes after the real Stop wrapper", async () => {
  const binding = { thread_id: "draft-thread", agent_id: agent.id, revision: agent.revision, cwd: "/repo", session_id: "raw", catalog, config_values: {} };
  let resolve!: (value: AcpThreadCatalog) => void;
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === "acp_agents") return [{ ...agent, revision: "changed" }];
    if (cmd === "acp_binding") return binding;
    if (cmd === "acp_thread_catalog") return new Promise<AcpThreadCatalog>(r => { resolve = r; });
    if (cmd === "agent_chat_stop_session") return;
    return [];
  });
  const p = props();
  const { container } = render(<TooltipProvider><Composer {...p} /></TooltipProvider>);
  const textarea = container.querySelector("textarea")!;
  await waitFor(() => expect(resolve).toBeTypeOf("function"));
  await act(async () => agentChatStopSession("acp", "draft-thread"));
  await act(async () => resolve({ catalog, live: true }));
  await screen.findByText(/launch configuration changed/);
  expect(useCustomAcp.getState().live["draft-thread"]).toBe(false);
  expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
  fireEvent.keyDown(textarea, { key: "Enter" });
  expect(p.onSubmit).not.toHaveBeenCalled();
  expect(container.querySelector("textarea")).toBe(textarea);
  expect(textarea.value).toBe("hello");
});
it("unblocks the actual Composer after renderer reload of an idle native session with a revised definition", async () => {
  const binding = { thread_id: "draft-thread", agent_id: agent.id, revision: agent.revision, cwd: "/repo", session_id: "raw", catalog, config_values: {} };
  invoke.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
    if (cmd === "acp_binding") return binding;
    if (cmd === "acp_agents") return [{ ...agent, revision: "changed" }];
    if (cmd === "acp_catalog" || cmd === "acp_thread_catalog") {
      expect(args).toEqual({ threadId: binding.thread_id });
      return cmd === "acp_catalog" ? catalog : { catalog, live: true };
    }
    return [];
  });
  const p = props();
  expect(useCustomAcp.getState().live).toEqual({});
  const { container } = render(<TooltipProvider><Composer {...p} /></TooltipProvider>);
  const textarea = container.querySelector("textarea")!;
  textarea.focus(); textarea.setSelectionRange(1, 3);
  await screen.findByText(/launch configuration changed/);
  await waitFor(() => expect(useCustomAcp.getState().restored["draft-thread"]).toBe(true));
  await waitFor(() => expect(screen.getByRole("button", { name: "Send" })).not.toBeDisabled());
  expect(container.querySelector("textarea")).toBe(textarea);
  expect(document.activeElement).toBe(textarea);
  expect(textarea.value).toBe("hello");
  fireEvent.keyDown(textarea, { key: "Enter" });
  expect(p.onSubmit).toHaveBeenCalledOnce();
  expect(invoke.mock.calls.some(([cmd]) => cmd === "agent_chat_start_session")).toBe(false);
});
it.each([360, 800])("hides conventional ACP controls at %ipx while retaining advertised runtime controls", async width => {
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({ width, height: 40, x: 0, y: 0, top: 0, left: 0, right: width, bottom: 40, toJSON: () => ({}) });
  const model = { id: "opaque", label: "Opaque", description: null, effort_levels: ["low", "ultrathink"], default_effort: "low", prompt_injected_effort_levels: [], context_window_options: [], supports_adaptive_thinking: false, supports_thinking_toggle: false, supports_fast_mode: false, supports_images: false, sub_provider: null, is_free: false };
  const live = { ...catalog, current_model: model.id, capabilities: { ...catalog.capabilities, models: [model] }, config_options: [{ id: " exact effort /ID ", category: "thought_level", name: "Agent reasoning", description: null, type: "select" as const, current_value: "low", options: [{ value: "low", name: "Low", description: null }, { value: "ultrathink", name: "Opaque high", description: null }] }] };
  const binding = { thread_id: "draft-thread", agent_id: agent.id, revision: agent.revision, cwd: "/repo", session_id: "s", catalog: live, config_values: {} };
  invoke.mockImplementation(async (cmd: string) => cmd === "acp_agents" ? [agent] : cmd === "acp_binding" ? binding : cmd === "acp_thread_catalog" ? { catalog: live, live: true } : cmd === "acp_set_config" ? live : []);
  const p = props();
  render(<TooltipProvider><Composer {...p} model={model.id} activeModel={model} effort="low" /></TooltipProvider>);
  await waitFor(() => expect(screen.getByRole("button", { name: "Send" })).not.toBeDisabled());
  fireEvent.click(screen.getByRole("button", { name: "Attach" }));
  expect(screen.queryByRole("button", { name: /^Reasoning:/ })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Attach" }));
  fireEvent.click(screen.getByTestId("multi-provider-model-picker-trigger"));
  fireEvent.change(await screen.findByLabelText("Agent reasoning"), { target: { value: "ultrathink" } });
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("acp_set_config", { threadId: "draft-thread", configId: " exact effort /ID ", value: "ultrathink" }));
  expect(p.onEffortChange).not.toHaveBeenCalled();
});
it("retains conventional native reasoning in a narrow attach menu", () => {
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({ width: 360 } as DOMRect);
  const model = { id: "native", label: "Native", description: null, effort_levels: ["low", "high"], default_effort: "low", prompt_injected_effort_levels: [], context_window_options: [], supports_adaptive_thinking: false, supports_thinking_toggle: false, supports_fast_mode: false, supports_images: false, sub_provider: null, is_free: false };
  const p = props();
  render(<TooltipProvider><Composer {...p} provider="claude" activeModel={model} effort="low" /></TooltipProvider>);
  fireEvent.click(screen.getByRole("button", { name: "Attach" }));
  fireEvent.click(screen.getByRole("button", { name: /^Reasoning:/ }));
  fireEvent.click(screen.getByRole("option", { name: /high/ }));
  expect(p.onEffortChange).toHaveBeenCalledWith("high");
});
it("does not offer local custom harnesses for an attached remote-host workspace", async () => {
  useAppStore.setState({ appState: { workspaces: [{ workspace_id: "remote", host_id: 7, attach_only: true }], active_workspace_id: "remote" } as never });
  render(<TooltipProvider><Composer {...props()} workspaceId="remote" provider="claude" /></TooltipProvider>);
  fireEvent.click(screen.getByTestId("multi-provider-model-picker-trigger"));
  expect(screen.queryByRole("button", { name: /Custom agents/ })).not.toBeInTheDocument();
});
it("blocks Enter until backend identity restoration finishes without replacing the textarea", async () => {
  let resolve!: (value: null) => void;
  const pending = new Promise<null>(r => { resolve = r; });
  invoke.mockImplementation(async (cmd: string) => cmd === "acp_binding" ? pending : cmd === "acp_agents" ? [agent] : []);
  const p = props();
  const { container } = render(<TooltipProvider><Composer {...p} /></TooltipProvider>);
  const textarea = container.querySelector("textarea")!;
  textarea.focus(); textarea.setSelectionRange(2, 4);
  fireEvent.keyDown(textarea, { key: "Enter" });
  expect(p.onSubmit).not.toHaveBeenCalled();
  await act(async () => resolve(null));
  await waitFor(() => expect(screen.getByRole("button", { name: "Send" })).not.toBeDisabled());
  expect(container.querySelector("textarea")).toBe(textarea);
  expect(document.activeElement).toBe(textarea);
  expect(textarea.value).toBe("hello");
  fireEvent.keyDown(textarea, { key: "Enter" });
  expect(p.onSubmit).toHaveBeenCalledOnce();
});
it("keeps a missing bound instance unavailable instead of selecting an enabled alternative", async () => {
  const binding = { thread_id: "draft-thread", agent_id: "removed", revision: "old", cwd: "/repo", session_id: "s", catalog: { ...catalog, agent_id: "removed" }, config_values: {} };
  invoke.mockImplementation(async (cmd: string) => cmd === "acp_binding" ? binding : cmd === "acp_agents" ? [agent] : cmd === "acp_thread_catalog" ? { catalog: binding.catalog, live: false } : []);
  const p = props();
  render(<TooltipProvider><Composer {...p} /></TooltipProvider>);
  await screen.findByText(/This custom agent was removed/);
  expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
  expect(useCustomAcp.getState().selections["draft-thread"]).toBe("removed");
});
it("keeps a natively started session usable after launch-config edits and remount, but not a cold restart", async () => {
  const binding = { thread_id: "draft-thread", agent_id: agent.id, revision: agent.revision, cwd: "/repo", session_id: "s", catalog, config_values: {} };
  let nativeLive = false;
  let revision = agent.revision;
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === "acp_binding") return binding;
    if (cmd === "acp_agents") return [{ ...agent, revision }];
    if (cmd === "acp_thread_catalog") {
      if (nativeLive) return { catalog, live: true };
      if (revision !== binding.revision) throw new Error("launch configuration changed");
      return { catalog: binding.catalog, live: false };
    }
    if (cmd === "agent_chat_start_session") {
      if (revision !== binding.revision) throw new Error("launch configuration changed");
      nativeLive = true; return "draft-thread";
    }
    return [];
  });
  await agentChatStartSession("pane", "acp", { thread_id: "draft-thread", cwd: "/repo", model: null, resume_cursor: null, permission_mode: null, additional_directories: [], env: null });
  revision = "edited";
  const p = props();
  let view = render(<TooltipProvider><Composer {...p} /></TooltipProvider>);
  await screen.findByText(/launch configuration changed/);
  await waitFor(() => expect(screen.getByRole("button", { name: "Send" })).not.toBeDisabled());
  view.unmount();
  view = render(<TooltipProvider><Composer {...p} /></TooltipProvider>);
  await screen.findByText(/launch configuration changed/);
  await waitFor(() => expect(screen.getByRole("button", { name: "Send" })).not.toBeDisabled());
  fireEvent.click(screen.getByRole("button", { name: "Send" }));
  expect(p.onSubmit).toHaveBeenCalledOnce();
  nativeLive = false;
  await expect(agentChatStartSession("pane", "acp", { thread_id: "draft-thread", cwd: "/repo", model: null, resume_cursor: { resume: "s" }, permission_mode: null, additional_directories: [], env: null })).rejects.toThrow("launch configuration changed");
});
it("blocks sending staged images when the selected harness advertises text-only support", async () => {
  render(<TooltipProvider><Composer {...props()} modelSupportsImages={true} stagedAttachments={[{ id: "image", kind: "image", ref: "image", metadata: { label: "fixture.png" } }]} /></TooltipProvider>);
  await waitFor(() => expect(useCustomAcp.getState().restored["draft-thread"]).toBe(true));
  expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
  expect(screen.getByText(/Remove image attachments/)).toBeInTheDocument();
});
it("selects a custom instance and its null default through the actual unified picker", async () => {
  const onChange = vi.fn();
  render(<TooltipProvider><MultiProviderModelPicker hermesThreadId="draft-thread" provider="claude" model={null} onProviderModelChange={onChange} /></TooltipProvider>);
  fireEvent.click(screen.getByTestId("multi-provider-model-picker-trigger"));
  fireEvent.click(await screen.findByRole("button", { name: /Custom agents/ }));
  fireEvent.click(await screen.findByRole("button", { name: "Use agent default" }));
  expect(onChange).toHaveBeenCalledWith("acp", null);
  expect(invoke).not.toHaveBeenCalledWith("acp_probe", expect.anything());
});
