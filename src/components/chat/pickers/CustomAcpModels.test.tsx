import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { CustomAcpModels } from "./CustomAcpModels";
import { acpCatalogKey, useCustomAcp } from "@/stores/custom-acp-store";
import type { AcpAgent, AcpCatalog } from "@/tauri/custom-acp";
const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
const agent: AcpAgent = { id: "instance-a", name: "My agent", executable: "cli", args: [], environment: {}, enabled: true, auth_method: null, revision: "r" };
const catalog: AcpCatalog = { agent_id: agent.id, agent_name: agent.name, capabilities: { models: [], permission_modes: [], effort_granularity: "per_session", effort_label_map: {}, permission_granularity: "per_session", default_permission_mode: null }, config_options: [], current_model: null, supports_resume: false, supports_images: false, auth_methods: [] };
beforeEach(() => {
  useCustomAcp.setState({ agents: [agent], selections: { draft: agent.id }, bindings: {}, catalogs: { [acpCatalogKey(agent.id, agent.revision)]: catalog }, threadCatalogs: {}, restored: {}, errors: {}, busy: {} });
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string) => cmd === "acp_agents" ? [agent] : cmd === "acp_binding" ? null : catalog);
});
afterEach(cleanup);
it("offers the agent default as null even with an empty authoritative catalog and never auto-probes", async () => {
  const onSelect = vi.fn();
  render(<CustomAcpModels threadId="draft" model={null} onSelect={onSelect} />);
  fireEvent.click(await screen.findByRole("button", { name: "Use agent default" }));
  expect(onSelect).toHaveBeenCalledWith(null);
  expect(invoke).not.toHaveBeenCalledWith("acp_probe", expect.anything());
});
it("uses exact opaque model IDs without interpreting the native default alias", async () => {
  const model = { id: " Default /A:B ", label: "Opaque model", description: null, effort_levels: [], default_effort: null, prompt_injected_effort_levels: [], context_window_options: [], supports_adaptive_thinking: false, supports_thinking_toggle: false, supports_fast_mode: false, supports_images: false, sub_provider: null, is_free: false };
  useCustomAcp.setState({ catalogs: { [acpCatalogKey(agent.id, agent.revision)]: { ...catalog, capabilities: { ...catalog.capabilities, models: [model] } } } });
  const onSelect = vi.fn();
  render(<CustomAcpModels threadId="draft" model={null} onSelect={onSelect} />);
  fireEvent.click(await screen.findByRole("button", { name: /Opaque model/ }));
  expect(onSelect).toHaveBeenCalledWith(model.id);
});
it("locks backend-bound identity and applies exact runtime reasoning/config values", async () => {
  const live = { ...catalog, config_options: [{ id: "Reason /Case", category: "thought_level", name: "Reasoning", description: null, type: "select" as const, current_value: "low", options: [{ value: " Exact:HIGH ", name: "High", description: null }, { value: "low", name: "Low", description: null }] }, { id: "toggle", category: null, name: "Thinking", description: null, type: "boolean" as const, current_value: false, options: [] }] };
  const binding = { thread_id: "bound", agent_id: agent.id, revision: agent.revision, cwd: "/repo", session_id: "session", catalog: live, config_values: {} };
  invoke.mockImplementation(async (cmd: string) => cmd === "acp_agents" ? [agent] : cmd === "acp_binding" ? binding : cmd === "acp_thread_catalog" ? { catalog: live, live: true } : live);
  render(<CustomAcpModels threadId="bound" model={null} onSelect={vi.fn()} />);
  await waitFor(() => expect(screen.getByLabelText("Custom agent")).toBeDisabled());
  fireEvent.change(await screen.findByLabelText("Reasoning"), { target: { value: " Exact:HIGH " } });
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("acp_set_config", { threadId: "bound", configId: "Reason /Case", value: " Exact:HIGH " }));
  await waitFor(() => expect(screen.getByLabelText("Thinking")).not.toBeDisabled());
  fireEvent.click(screen.getByLabelText("Thinking"));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("acp_set_config", { threadId: "bound", configId: "toggle", value: true }));
});

const effortCatalog: AcpCatalog = { ...catalog, config_options: [{ id: " effort /Case ", category: "thought_level", name: "Agent effort", description: null, type: "select", current_value: "low", options: [{ value: "low", name: "Low", description: null }, { value: "ultrathink", name: "Opaque high", description: null }] }] };
function effortTransport() {
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === "acp_agents") return [agent];
    if (cmd === "acp_binding") return { thread_id: "bound", agent_id: agent.id, revision: agent.revision, cwd: "/repo", session_id: "s", catalog: effortCatalog, config_values: {} };
    if (cmd === "acp_thread_catalog") return { catalog: effortCatalog, live: true };
    throw new Error(`Unexpected command: ${cmd}`);
  });
  return invoke.getMockImplementation()!;
}
it("retains accepted effort and exposes rejected configuration without a generic persistence write", async () => {
  const fallback = effortTransport();
  invoke.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
    if (cmd === "acp_set_config") { expect(args).toEqual({ threadId: "bound", configId: " effort /Case ", value: "ultrathink" }); throw new Error("effort retired"); }
    return fallback(cmd, args);
  });
  render(<CustomAcpModels threadId="bound" model={null} onSelect={vi.fn()} />);
  await waitFor(() => expect(screen.getByLabelText("Agent effort")).not.toBeDisabled());
  const reads = invoke.mock.calls.filter(([cmd]) => cmd === "acp_thread_catalog").length;
  fireEvent.change(screen.getByLabelText("Agent effort"), { target: { value: "ultrathink" } });
  expect(await screen.findByRole("alert")).toHaveTextContent("effort retired");
  expect(screen.getByLabelText("Agent effort")).toHaveValue("low");
  expect(useCustomAcp.getState().threadCatalogs.bound).toEqual(effortCatalog);
  expect(invoke.mock.calls.filter(([cmd]) => cmd === "acp_thread_catalog")).toHaveLength(reads);
  expect(invoke.mock.calls.filter(([cmd]) => cmd === "agent_chat_update_session_config")).toEqual([]);
});
it("waits for setter acknowledgement and catalog readback instead of painting a stale effort response", async () => {
  const fallback = effortTransport();
  let acknowledge!: (value: AcpCatalog) => void;
  let readback!: (value: AcpCatalog) => void;
  let accepted = false;
  invoke.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
    if (cmd === "acp_set_config") {
      expect(args).toEqual({ threadId: "bound", configId: " effort /Case ", value: "ultrathink" });
      const response = await new Promise<AcpCatalog>(r => { acknowledge = r; }); accepted = true; return response;
    }
    if (cmd === "acp_thread_catalog" && accepted) return new Promise<AcpCatalog>(r => { readback = r; }).then(catalog => ({ catalog, live: true }));
    return fallback(cmd, args);
  });
  render(<CustomAcpModels threadId="bound" model={null} onSelect={vi.fn()} />);
  await waitFor(() => expect(screen.getByLabelText("Agent effort")).not.toBeDisabled());
  fireEvent.change(screen.getByLabelText("Agent effort"), { target: { value: "ultrathink" } });
  expect(screen.getByLabelText("Agent effort")).toHaveValue("low");
  expect(screen.getByLabelText("Agent effort")).toBeDisabled();
  const stale = { ...effortCatalog, config_options: [{ ...effortCatalog.config_options[0], current_value: "ultrathink" }] };
  await act(async () => acknowledge(stale));
  expect(screen.getByLabelText("Agent effort")).toHaveValue("low");
  expect(screen.getByLabelText("Agent effort")).toBeDisabled();
  const authoritative = { ...effortCatalog, config_options: [{ ...effortCatalog.config_options[0], current_value: " Exact/Accepted ", options: [{ value: " Exact/Accepted ", name: "Accepted", description: null }] }] };
  await act(async () => readback(authoritative));
  await waitFor(() => expect(screen.getByLabelText("Agent effort")).not.toBeDisabled());
  expect(screen.getByLabelText("Agent effort")).toHaveValue(" Exact/Accepted ");
  expect(screen.queryByRole("option", { name: "Opaque high" })).not.toBeInTheDocument();
  expect(useCustomAcp.getState().threadCatalogs.bound).toEqual(authoritative);
});
