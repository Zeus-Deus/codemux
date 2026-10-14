import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { CustomAcpDetail } from "./custom-acp-settings";
import { useCustomAcp } from "@/stores/custom-acp-store";
import type { AcpAgent, AcpAgentInput } from "@/tauri/custom-acp";
const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
let agents: AcpAgent[];
const saved: AcpAgent = { id: "backend-id", name: "Custom", executable: "/harness", args: ["two words", "$(literal)"], environment: { TOKEN: null }, auth_method: null, enabled: true, revision: "r1" };
beforeEach(() => {
  agents = [];
  useCustomAcp.setState({ agents: null, selections: {}, bindings: {}, catalogs: {}, threadCatalogs: {}, restored: {}, busy: {}, errors: {} });
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string, payload?: { input: AcpAgentInput; agentId: string }) => {
    if (cmd === "acp_agents") return agents;
    if (cmd === "acp_save_agent") { agents = [{ ...saved, ...payload!.input, id: payload!.input.id ?? saved.id }]; return agents[0]; }
    if (cmd === "acp_delete_agent") { agents = agents.filter(a => a.id !== payload!.agentId); return; }
    if (cmd === "acp_probe") return { agent_id: saved.id, agent_name: saved.name, config_options: [], current_model: null, supports_images: false, supports_resume: true, auth_methods: [], capabilities: { models: [], permission_modes: [] } };
  });
});
afterEach(cleanup);
it("adds a local harness with literal argument rows, then probes only by explicit intent", async () => {
  const onSaved = vi.fn();
  const view = render(<CustomAcpDetail agentId={null} onSaved={onSaved} onCancelNew={() => {}} />);
  fireEvent.change(screen.getByLabelText("Agent name"), { target: { value: "My harness" } });
  fireEvent.change(screen.getByLabelText("Executable"), { target: { value: "/opt/my harness" } });
  fireEvent.click(screen.getByRole("button", { name: "Add argument" }));
  fireEvent.change(screen.getByLabelText("Argument 1"), { target: { value: "two words $(literal)" } });
  fireEvent.click(screen.getByRole("button", { name: "Add environment variable" }));
  fireEvent.change(screen.getByLabelText("Environment key 1"), { target: { value: "TOKEN" } });
  fireEvent.change(screen.getByLabelText("Environment value 1"), { target: { value: "secret-fixture" } });
  fireEvent.click(screen.getByRole("button", { name: "Save agent" }));
  await waitFor(() => expect(onSaved).toHaveBeenCalledWith(saved.id));
  expect(invoke).toHaveBeenCalledWith("acp_save_agent", { input: { name: "My harness", executable: "/opt/my harness", args: ["two words $(literal)"], environment: { TOKEN: "secret-fixture" }, enabled: true, auth_method: null } });
  expect(invoke).not.toHaveBeenCalledWith("acp_probe", expect.anything());
  expect(JSON.stringify(useCustomAcp.getState())).not.toContain("secret-fixture");
  // The page then shows the saved agent's detail.
  view.rerender(<CustomAcpDetail key={saved.id} agentId={saved.id} onSaved={onSaved} onCancelNew={() => {}} />);
  fireEvent.click(await screen.findByRole("button", { name: "Probe My harness" }));
  await screen.findByText(/No models advertised/);
  expect(invoke).toHaveBeenCalledWith("acp_probe", { agentId: saved.id, cwd: null });
});
it("retains redacted environment values as null and removes only deleted keys", async () => {
  agents = [saved];
  render(<CustomAcpDetail agentId={saved.id} onSaved={() => {}} onCancelNew={() => {}} />);
  fireEvent.click(await screen.findByRole("button", { name: "Edit Custom" }));
  expect(screen.getByLabelText("Environment value 1")).toHaveValue("");
  fireEvent.change(screen.getByLabelText("Agent name"), { target: { value: "Renamed" } });
  fireEvent.click(screen.getByRole("button", { name: "Save agent" }));
  await screen.findByRole("button", { name: "Edit Renamed" });
  expect(invoke).toHaveBeenCalledWith("acp_save_agent", { input: expect.objectContaining({ id: saved.id, args: saved.args, environment: { TOKEN: null } }) });
  fireEvent.click(screen.getByRole("button", { name: "Edit Renamed" }));
  fireEvent.click(screen.getByRole("button", { name: "Remove environment variable 1" }));
  fireEvent.click(screen.getByRole("button", { name: "Save agent" }));
  await waitFor(() => expect(invoke).toHaveBeenLastCalledWith("acp_agents"));
  expect(invoke).toHaveBeenCalledWith("acp_save_agent", { input: expect.objectContaining({ environment: {} }) });
});
it("the switch saves only the enabled flag and keeps saved secrets", async () => {
  agents = [saved];
  render(<CustomAcpDetail agentId={saved.id} onSaved={() => {}} onCancelNew={() => {}} />);
  fireEvent.click(await screen.findByRole("switch", { name: "Use Custom" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("acp_save_agent", {
    input: { id: saved.id, name: saved.name, executable: saved.executable, args: saved.args, environment: { TOKEN: null }, enabled: false, auth_method: null },
  }));
});
it("keeps the switch inert while the editor has unsaved edits", async () => {
  agents = [saved];
  const onLockChange = vi.fn();
  render(<CustomAcpDetail agentId={saved.id} onSaved={() => {}} onCancelNew={() => {}} onLockChange={onLockChange} />);
  fireEvent.click(await screen.findByRole("button", { name: "Edit Custom" }));
  fireEvent.change(screen.getByLabelText("Executable"), { target: { value: "/edited" } });
  const toggle = screen.getByRole("switch", { name: "Use Custom" });
  expect(toggle).toBeDisabled();
  fireEvent.click(toggle);
  expect(invoke).not.toHaveBeenCalledWith("acp_save_agent", expect.anything());
  expect(screen.getByLabelText("Executable")).toHaveValue("/edited");
  // The page hears the lock so it can disable the list switch too.
  expect(onLockChange).toHaveBeenLastCalledWith(true);
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  await waitFor(() => expect(onLockChange).toHaveBeenLastCalledWith(false));
  expect(screen.getByRole("switch", { name: "Use Custom" })).toBeEnabled();
});
it("keeps the switch inert while a save is pending", async () => {
  agents = [saved];
  let finish: () => void = () => {};
  render(<CustomAcpDetail agentId={saved.id} onSaved={() => {}} onCancelNew={() => {}} />);
  const toggle = await screen.findByRole("switch", { name: "Use Custom" });
  invoke.mockImplementation(async (cmd: string, payload?: { input: AcpAgentInput }) => {
    if (cmd === "acp_agents") return agents;
    if (cmd === "acp_save_agent") { await new Promise<void>(r => { finish = r; }); agents = [{ ...saved, ...payload!.input, id: saved.id }]; return agents[0]; }
  });
  fireEvent.click(toggle);
  await waitFor(() => expect(toggle).toBeDisabled());
  fireEvent.click(toggle);
  expect(invoke.mock.calls.filter(([cmd]) => cmd === "acp_save_agent")).toHaveLength(1);
  finish();
  await waitFor(() => expect(screen.getByRole("switch", { name: "Use Custom" })).toBeEnabled());
});
it("requires explicit delete confirmation and verifies removal", async () => {
  agents = [saved];
  render(<CustomAcpDetail agentId={saved.id} onSaved={() => {}} onCancelNew={() => {}} />);
  fireEvent.click(await screen.findByRole("button", { name: "Delete Custom" }));
  expect(invoke).not.toHaveBeenCalledWith("acp_delete_agent", expect.anything());
  fireEvent.click(screen.getByRole("button", { name: "Confirm delete" }));
  await screen.findByText("This custom agent was removed.");
  expect(invoke).toHaveBeenCalledWith("acp_delete_agent", { agentId: saved.id });
});
