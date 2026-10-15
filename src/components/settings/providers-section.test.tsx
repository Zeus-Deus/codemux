import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { AcpAgent, AcpAgentInput } from "@/tauri/custom-acp";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
vi.mock("@/tauri/events", () => ({
  onCustomAcpChanged: vi.fn().mockResolvedValue(() => {}),
  onCustomAcpCatalogChanged: vi.fn().mockResolvedValue(() => {}),
}));

import { ProvidersSection } from "./providers-section";
import { useCustomAcp } from "@/stores/custom-acp-store";
import { useAcpMutationLock } from "@/stores/acp-mutation-lock";
import { useSettingsStore } from "@/stores/settings-store";

const saved: AcpAgent = { id: "agent-1", name: "Kimi", executable: "/bin/kimi", args: ["--acp"], environment: { KEY: null }, auth_method: null, enabled: true, revision: "r1" };
let agents: AcpAgent[];
let saves: AcpAgentInput[];
/** Holds each save until the test releases it, like a slow backend. */
let release: Array<() => void>;

beforeEach(() => {
  agents = [saved];
  saves = [];
  release = [];
  useCustomAcp.setState({ agents: null, selections: {}, bindings: {}, catalogs: {}, threadCatalogs: {}, restored: {}, busy: {}, errors: {} });
  useAcpMutationLock.setState({ pending: {} });
  useSettingsStore.setState({ settings: {} });
  invoke.mockReset();
  invoke.mockImplementation(async (cmd: string, payload?: { input: AcpAgentInput }) => {
    if (cmd === "acp_agents") return agents;
    if (cmd === "acp_save_agent") {
      saves.push(payload!.input);
      await new Promise<void>((r) => release.push(r));
      // The backend replaces the whole definition.
      agents = [{ ...saved, ...payload!.input, id: saved.id, revision: `r${saves.length + 1}`, environment: Object.fromEntries(Object.keys(payload!.input.environment).map((k) => [k, null])) }];
      return agents[0];
    }
    if (cmd === "hermes_profiles") return [];
    if (cmd === "agent_chat_provider_health") return { provider: "claude", status: "ready", installed: true, message: null, version: "1.0.0" };
    if (cmd === "get_chatgpt_status") return { phase: "disconnected", attemptId: null, email: null, error: null, profiles: [], activeProfileId: null, welcomePending: false, installed: true };
    if (cmd === "get_local_workbench") return false;
    return null;
  });
});
afterEach(cleanup);

const nav = () => screen.getByRole("navigation", { name: "Providers" });
const listSwitch = () => within(nav()).getByRole("switch", { name: "Use Kimi" });

async function openAgent() {
  render(<ProvidersSection />);
  fireEvent.click(await within(nav()).findByRole("button", { name: /^Kimi/ }));
  await screen.findByRole("region", { name: "Kimi" });
}

it("a pending list toggle blocks Edit and Save until it settles", async () => {
  await openAgent();
  fireEvent.click(listSwitch());
  await waitFor(() => expect(saves).toHaveLength(1));
  expect(saves[0].enabled).toBe(false);

  // Everything that writes this agent waits for the toggle.
  expect(screen.getByRole("button", { name: "Edit Kimi" })).toBeDisabled();
  const panel = screen.getByRole("region", { name: "Kimi" });
  expect(within(panel).getByRole("switch", { name: "Use Kimi" })).toBeDisabled();
  expect(listSwitch()).toBeDisabled();

  await act(async () => { release.shift()!(); });
  await waitFor(() => expect(screen.getByRole("button", { name: "Edit Kimi" })).toBeEnabled());

  // The editor now starts from the saved (disabled) definition, so saving
  // it cannot turn the agent back on.
  fireEvent.click(screen.getByRole("button", { name: "Edit Kimi" }));
  fireEvent.change(screen.getByLabelText("Executable"), { target: { value: "/bin/kimi2" } });
  fireEvent.click(screen.getByRole("button", { name: "Save agent" }));
  await waitFor(() => expect(saves).toHaveLength(2));
  expect(saves[1]).toMatchObject({ executable: "/bin/kimi2", enabled: false });
  await act(async () => { release.shift()!(); });
});

it("a pending editor save keeps the list switch locked after navigating away", async () => {
  await openAgent();
  fireEvent.click(screen.getByRole("button", { name: "Edit Kimi" }));
  // While editing, the list switch cannot save over the editor.
  expect(listSwitch()).toBeDisabled();
  fireEvent.change(screen.getByLabelText("Executable"), { target: { value: "/bin/kimi2" } });
  fireEvent.click(screen.getByRole("button", { name: "Add environment variable" }));
  fireEvent.change(screen.getByLabelText("Environment key 2"), { target: { value: "NEW" } });
  fireEvent.change(screen.getByLabelText("Environment value 2"), { target: { value: "v" } });
  fireEvent.click(screen.getByRole("button", { name: "Save agent" }));
  await waitFor(() => expect(saves).toHaveLength(1));

  // Leave the agent's panel while its save is still in flight.
  fireEvent.click(within(nav()).getByRole("button", { name: /^Claude/ }));
  await screen.findByRole("heading", { name: "Claude" });
  expect(listSwitch()).toBeDisabled();
  fireEvent.click(listSwitch());
  expect(saves).toHaveLength(1);

  await act(async () => { release.shift()!(); });
  await waitFor(() => expect(listSwitch()).toBeEnabled());
  expect(agents[0]).toMatchObject({ executable: "/bin/kimi2" });
  expect(Object.keys(agents[0].environment).sort()).toEqual(["KEY", "NEW"]);
});
