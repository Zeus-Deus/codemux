import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, renderHook, screen, waitFor } from "@testing-library/react";
import { AgentChatPane } from "./AgentChatPane";
import { TooltipProvider } from "@/components/ui/tooltip";
import { useAgentChatSessionActions } from "@/hooks/use-agent-chat-session-actions";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { useAppStore } from "@/stores/app-store";
import { useCustomAcp } from "@/stores/custom-acp-store";
import { useProviderRuntimeIntent } from "@/stores/provider-runtime-intent-store";
import type { AcpCatalog } from "@/tauri/custom-acp";
import type { AgentChatSessionRecord } from "@/tauri/commands";
import { materializeAndSend, materializeWithPreset, type MaterializeActions } from "@/lib/agent-chat/materialize";
import type { ChatDraft } from "@/stores/chat-draft-store";
import type { TerminalPreset } from "@/tauri/types";

const { invoke, listen } = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn(async () => () => {}) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke, convertFileSrc: (p: string) => p, Channel: class { id = 1; constructor(public onmessage: (payload: unknown) => void) {} } }));
vi.mock("@tauri-apps/api/event", () => ({ listen, emit: vi.fn() }));
// Replace only virtual layout; the pane, Composer, picker, stores, history
// actions, command wrappers and native-event wrappers are production code.
vi.mock("@legendapp/list/react", async () => {
  const React = await import("react");
  return { LegendList: React.forwardRef(function List(props: Record<string, any>, ref) {
    const node = React.useRef<HTMLDivElement>(null);
    React.useImperativeHandle(ref, () => ({ getScrollableNode: () => node.current, getState: () => ({ scroll: 0, scrollLength: 500, isAtEnd: true, listen: () => () => {} }), scrollToEnd: () => Promise.resolve(), scrollToIndex: () => Promise.resolve(), scrollToOffset: () => Promise.resolve() }));
    return <div ref={node}>{props.ListHeaderComponent}{props.data.map((item: unknown, index: number) => <React.Fragment key={props.keyExtractor(item, index)}>{props.renderItem({ item, index })}</React.Fragment>)}{props.ListFooterComponent}</div>;
  }) };
});
const agent = { id: "opaque-agent", name: "Test ACP", executable: "fixture", args: [], environment: {}, enabled: true, auth_method: null, revision: "r1" };
const catalog: AcpCatalog = { agent_id: agent.id, agent_name: agent.name, capabilities: { models: [], effort_granularity: "per_session", effort_label_map: {}, permission_modes: [], default_permission_mode: null, permission_granularity: "per_session" }, config_options: [{ id: " raw thought ID ", name: "Harness thought level", description: null, category: "thought_level", type: "select", current_value: "ultrathink", options: [{ value: "ultrathink", name: "Opaque effort", description: null }] }], current_model: null, supports_images: false, supports_resume: true, auth_methods: [] };
const rawSessionId = " raw/session:opaque ";
const sdkSessionId = JSON.stringify(["acp", agent.id, agent.revision, rawSessionId]);
const record: AgentChatSessionRecord = { thread_id: "history-thread", sdk_session_id: sdkSessionId, workspace_id: "ws", cwd: "/repo", provider: "acp", title: "History", created_at: "2026-01-01", last_active_at: "2026-01-01", model: null, effort: "ultrathink", context_window: null, permission_mode: null };
const binding = { thread_id: record.thread_id, agent_id: agent.id, revision: agent.revision, cwd: "/repo", session_id: rawSessionId, catalog, config_values: { " raw thought ID ": "ultrathink" } };
// Native transport seam only: validate the real cursor and advertised opaque
// effort contract. These checks do not execute or certify the Rust backend.
function validateEffort(value: unknown, current: AcpCatalog = catalog) {
  if (value == null) return;
  const option = current.config_options.find(o => o.category === "thought_level");
  if (!option || option.type !== "select" || !option.options.some(o => o.value === value)) throw new Error("Effort not advertised by harness");
}
const pane = { kind: "agent_chat" as const, pane_id: "pane-acp", thread_id: record.thread_id, provider: "acp" as const, cwd: "/repo", title: "ACP" };
beforeEach(() => {
  useAgentChatStore.setState({ threads: {} });
  useAppStore.setState({ homeDir: "/test-home", appState: { active_workspace_id: "ws", workspaces: [{ workspace_id: "ws", cwd: "/repo", project_root: "/repo", surfaces: [{ root: pane }] }] } as never });
  useCustomAcp.setState({ agents: null, selections: {}, bindings: {}, catalogs: {}, threadCatalogs: {}, restored: {}, errors: {}, busy: {}, live: {} });
  useProviderRuntimeIntent.getState().reset();
  useProviderRuntimeIntent.getState().observe("acp");
  invoke.mockReset();
  let startedDraft = false;
  invoke.mockImplementation(async (cmd: string, args?: Record<string, any>) => {
    switch (cmd) {
      case "acp_agents": return [agent];
      case "acp_binding": return args!.threadId === "draft-thread" ? (startedDraft ? { ...binding, thread_id: "draft-thread" } : null) : binding;
      case "acp_catalog": return catalog;
      case "acp_thread_catalog": return { catalog, live: args!.threadId === "draft-thread" ? startedDraft : true };
      case "create_empty_workspace": return "ws";
      case "agent_chat_create_pane": return pane.pane_id;
      case "agent_chat_start_session": {
        if (args!.provider === "acp") {
          expect(args!.input.extra).toEqual({ acp_agent_id: agent.id });
          expect(args!.input.model).toBeNull();
          if (args!.input.resume_cursor !== null) expect(args!.input.resume_cursor).toEqual({ resume: sdkSessionId });
          validateEffort(args!.input.effort);
        }
        startedDraft = true; return args!.input.thread_id;
      }
      case "agent_chat_get_session": return record;
      case "agent_chat_list_messages_tail": return { rows: [], total_rows: 0, complete: true };
      case "agent_chat_thread_head_id": return null;
      case "agent_chat_turn_active": return false;
      case "agent_chat_send_turn":
        if (args!.provider === "acp") validateEffort(args!.input.effort_override);
        return { turn_id: "turn", queued_id: null };
      case "generate_branch_name": return "fixture-title";
      case "agent_chat_provider_health": return { status: "ready", installed: true, message: null, version: null };
      case "list_chat_provider_capabilities": return catalog.capabilities;
      default: return [];
    }
  });
});
afterEach(cleanup);
it.each(["ultrathink", null])("sends unchanged ACP text and attachments after native-contract history restores effort %s", async effort => {
  // Null is valid startup intent; native readback still reports the harness's
  // accepted thought-level default in its history row and catalog.
  const actions = renderHook(() => useAgentChatSessionActions({ ...pane, thread_id: null }));
  await act(async () => actions.result.current.handleSelect({ ...record, effort }));
  expect(invoke).toHaveBeenCalledWith("agent_chat_start_session", expect.objectContaining({ provider: "acp", input: expect.objectContaining({ model: null, effort, resume_cursor: { resume: sdkSessionId }, extra: { acp_agent_id: agent.id } }) }));
  expect(JSON.parse(sdkSessionId)).toEqual(["acp", agent.id, agent.revision, rawSessionId]);
  expect(useCustomAcp.getState().bindings[record.thread_id].session_id).toBe(rawSessionId);
  expect(useAgentChatStore.getState().threads[record.thread_id].effort).toBe(effort);
  useAgentChatStore.getState().setInputDraft(record.thread_id, "Keep user ultrathink text @a.ts");
  useAgentChatStore.getState().addStagedAttachment(record.thread_id, { id: "file", kind: "file", ref: "/repo/a.ts", resolvedContent: "const answer = 42;", metadata: { label: "a.ts" } });
  render(<TooltipProvider><AgentChatPane pane={pane} /></TooltipProvider>);
  await waitFor(() => expect(screen.getByRole("button", { name: "Send" })).not.toBeDisabled());
  fireEvent.click(screen.getByRole("button", { name: "Send" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("agent_chat_send_turn", expect.objectContaining({ provider: "acp", input: expect.objectContaining({ display_text: "Keep user ultrathink text @a.ts" }) })));
  const input = invoke.mock.calls.find(([cmd]) => cmd === "agent_chat_send_turn")![1].input;
  expect(input.text).not.toContain("Ultrathink:");
  expect(input.text).toContain("Keep user ultrathink text @a.ts");
  expect(input.text).toContain("const answer = 42;");
  expect(input.effort_override).toBeNull();
  expect(useAgentChatStore.getState().threads[record.thread_id].effort).toBe(record.effort);
  expect(record.effort).toBe(catalog.config_options[0].current_value);
  expect(invoke).toHaveBeenCalledWith("acp_thread_catalog", { threadId: record.thread_id });
  expect(useCustomAcp.getState().threadCatalogs[record.thread_id]).toEqual(catalog);
});

it.each(["draft", "preset"].flatMap(path => ["ultrathink", null].map(effort => ({ path, effort }))))("preserves advertised ACP $effort and null defaults in the $path first-send path", async ({ path, effort }) => {
  const draft: ChatDraft = { draftId: "draft" as ChatDraft["draftId"], createdAt: "2026-01-01", target: path === "preset" ? { kind: "project", projectPath: "/repo" } : { kind: "existing_workspace", workspaceId: "ws" }, provider: "acp", model: null, effort, contextWindow: null, permissionMode: null, mode: "default", inputDraft: "user ultrathink", threadId: "draft-thread", promoting: false, promotedTo: null, materializedTo: null, lastSendError: null };
  useCustomAcp.getState().select(draft.threadId, agent.id);
  const store = useAgentChatStore.getState();
  const actions: MaterializeActions = { markPromoting: vi.fn(), markMaterialized: vi.fn(), markPromoted: vi.fn(), markSendFailed: vi.fn(), ensureThread: store.ensureThread, appendUserMessage: store.appendUserMessage, removeUserMessageByNonce: store.removeUserMessageByNonce, setModel: store.setModel, setPermissionMode: store.setPermissionMode, setSessionLaunchMode: store.setSessionLaunchMode, setEffort: store.setEffort, setContextWindow: store.setContextWindow, setFastMode: store.setFastMode, setMode: store.setMode };
  const preset: TerminalPreset = { id: "test", name: "Test", description: null, commands: [], working_directory: null, launch_mode: "new_tab", icon: null, pinned: false, is_builtin: false, auto_run_on_workspace: false, auto_run_on_new_tab: false, kind: "chat_agent" };
  const result = path === "draft" ? await materializeAndSend(draft, draft.inputDraft, "/repo", actions, null, "Attached raw context") : await materializeWithPreset(draft, preset, draft.inputDraft, actions);
  expect(result.success).toBe(true);
  expect(invoke).toHaveBeenCalledWith("agent_chat_start_session", expect.objectContaining({ provider: "acp", input: expect.objectContaining({ model: null, effort, extra: { acp_agent_id: agent.id } }) }));
  expect(useCustomAcp.getState().bindings[draft.threadId]).toMatchObject({ agent_id: agent.id, session_id: rawSessionId, catalog, config_values: { " raw thought ID ": "ultrathink" } });
  const input = invoke.mock.calls.find(([cmd]) => cmd === "agent_chat_send_turn")![1].input;
  expect(input.effort_override).toBe(effort);
  expect(input.text).not.toContain("Ultrathink:");
  expect(input.text).toContain("user ultrathink");
  if (path === "draft") expect(input.text).toContain("Attached raw context");
  expect(input.display_text).toBe("user ultrathink");
  expect(useAgentChatStore.getState().threads[draft.threadId].effort).toBe(effort);
});

const modelOption = (id: string) => ({ id, label: id, description: null, effort_levels: [], default_effort: null, prompt_injected_effort_levels: [], context_window_options: [], supports_adaptive_thinking: false, supports_thinking_toggle: false, supports_fast_mode: false, supports_images: false, sub_provider: null, is_free: false });
const modelCatalog: AcpCatalog = { ...catalog, current_model: "model A", capabilities: { ...catalog.capabilities, models: [modelOption("model A"), modelOption("model B")] }, config_options: [{ id: "effort A", name: "Old effort", description: null, category: "thought_level", type: "select", current_value: "ultrathink", options: [{ value: "ultrathink", name: "Opaque effort", description: null }] }] };
function modelTransport() {
  const fallback = invoke.getMockImplementation()!;
  invoke.mockImplementation(async (cmd: string, args?: Record<string, any>) => {
    if (cmd === "acp_binding") return { ...binding, catalog: modelCatalog, config_values: { "effort A": "ultrathink" } };
    if (cmd === "acp_thread_catalog") return { catalog: modelCatalog, live: true };
    if (cmd === "agent_chat_get_session") return { ...record, model: "model A" };
    return fallback(cmd, args);
  });
  return invoke.getMockImplementation()!;
}
it("keeps a rejected advertised model out of the pane and reports the native setter error", async () => {
  const fallback = modelTransport();
  invoke.mockImplementation(async (cmd: string, args?: Record<string, any>) => {
    if (cmd === "agent_chat_set_model") throw new Error("model retired by harness");
    return fallback(cmd, args);
  });
  const { toast } = await import("@/lib/toast");
  const error = vi.spyOn(toast, "error");
  render(<TooltipProvider><AgentChatPane pane={pane} /></TooltipProvider>);
  await waitFor(() => expect(screen.getByTestId("multi-provider-model-picker-trigger")).not.toBeDisabled());
  fireEvent.click(screen.getByTestId("multi-provider-model-picker-trigger"));
  fireEvent.click(await screen.findByRole("button", { name: "model B" }));
  await waitFor(() => expect(error).toHaveBeenCalledWith(expect.stringContaining("model retired by harness")));
  expect(invoke).toHaveBeenCalledWith("agent_chat_set_model", { provider: "acp", threadId: record.thread_id, model: "model B" });
  expect(useAgentChatStore.getState().threads[record.thread_id].model).toBe("model A");
  expect(useCustomAcp.getState().threadCatalogs[record.thread_id].current_model).toBe("model A");
  expect(invoke.mock.calls.filter(([cmd]) => cmd === "agent_chat_update_session_config")).toEqual([]);
  expect(screen.getByTestId("multi-provider-model-picker-trigger")).toHaveTextContent("model A");
  error.mockRestore();
});
it("paints only the authoritative catalog after the actual model setter wrapper is acknowledged", async () => {
  const fallback = modelTransport();
  let acknowledge!: () => void;
  let readback!: (value: AcpCatalog) => void;
  let accepted = false;
  invoke.mockImplementation(async (cmd: string, args?: Record<string, any>) => {
    if (cmd === "agent_chat_set_model") { await new Promise<void>(r => { acknowledge = r; }); accepted = true; return; }
    if (cmd === "acp_thread_catalog" && accepted) return new Promise<AcpCatalog>(r => { readback = r; }).then(catalog => ({ catalog, live: true }));
    return fallback(cmd, args);
  });
  render(<TooltipProvider><AgentChatPane pane={pane} /></TooltipProvider>);
  await waitFor(() => expect(screen.getByTestId("multi-provider-model-picker-trigger")).not.toBeDisabled());
  fireEvent.click(screen.getByTestId("multi-provider-model-picker-trigger"));
  fireEvent.click(await screen.findByRole("button", { name: "model B" }));
  expect(useAgentChatStore.getState().threads[record.thread_id].model).toBe("model A");
  await act(async () => acknowledge());
  expect(useAgentChatStore.getState().threads[record.thread_id].model).toBe("model A");
  const updated = { ...modelCatalog, current_model: "model B", config_options: [{ ...modelCatalog.config_options[0], id: "effort B", name: "New effort", current_value: " exact/new ", options: [{ value: " exact/new ", name: "New accepted", description: null }] }] };
  await act(async () => readback(updated));
  await waitFor(() => expect(screen.getByTestId("multi-provider-model-picker-trigger")).toHaveTextContent("model B"));
  expect(await screen.findByLabelText("New effort")).toHaveValue(" exact/new ");
  expect(screen.queryByLabelText("Old effort")).not.toBeInTheDocument();
  expect(useCustomAcp.getState().threadCatalogs[record.thread_id]).toEqual(updated);
  expect(invoke.mock.calls.filter(([cmd]) => cmd === "agent_chat_update_session_config")).toEqual([]);
});

it("preserves Claude ultrathink prompt semantics through the same real pane send path", async () => {
  const fallback = invoke.getMockImplementation()!;
  invoke.mockImplementation(async (cmd: string, args?: Record<string, any>) => cmd === "agent_chat_get_session" ? { ...record, provider: "claude" } : fallback(cmd, args));
  const store = useAgentChatStore.getState();
  store.ensureThread(record.thread_id);
  store.setEffort(record.thread_id, "ultrathink");
  store.setInputDraft(record.thread_id, "Native user text");
  render(<TooltipProvider><AgentChatPane pane={{ ...pane, provider: "claude" }} /></TooltipProvider>);
  await waitFor(() => expect(screen.getByRole("button", { name: "Send" })).not.toBeDisabled());
  fireEvent.click(screen.getByRole("button", { name: "Send" }));
  await waitFor(() => expect(invoke.mock.calls.some(([cmd]) => cmd === "agent_chat_send_turn")).toBe(true));
  const call = invoke.mock.calls.find(([cmd]) => cmd === "agent_chat_send_turn")![1];
  expect(call.provider).toBe("claude");
  expect(call.input.text).toBe("Ultrathink:\nNative user text");
});
