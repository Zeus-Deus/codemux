/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { TooltipProvider } from "@/components/ui/tooltip";
import { useSettingsStore } from "@/stores/settings-store";

const mocks = vi.hoisted(() => ({ capabilities: vi.fn(), setSetting: vi.fn() }));
vi.mock("@/tauri/workflows", async (original) => ({
  ...await original<typeof import("@/tauri/workflows")>(),
  workflowCapabilities: mocks.capabilities,
}));
vi.mock("@/tauri/commands", async (original) => ({
  ...await original<typeof import("@/tauri/commands")>(),
  dbSetSetting: mocks.setSetting,
}));
import { WorkflowsSection } from "./workflows-section";

const mount = () => render(<TooltipProvider><WorkflowsSection /></TooltipProvider>);

beforeEach(() => {
  vi.clearAllMocks();
  mocks.setSetting.mockResolvedValue(undefined);
  useSettingsStore.setState({ settings: {} });
  mocks.capabilities.mockResolvedValue([
    { provider: "claude", live: true, read_only: true, write: true, reason: null },
    { provider: "codex", live: true, read_only: true, write: true, reason: null },
    { provider: "opencode", live: true, read_only: true, write: true, requires_model: true, reason: null },
    { provider: "grok", live: false, read_only: false, write: false, reason: "No fan-out boundary" },
  ]);
});
afterEach(() => cleanup());

describe("Settings → Agents → Workflows", () => {
  it("defaults to same-provider runs and saves a switch to mixing providers", () => {
    mount();
    const same = screen.getByRole("radio", { name: /Same provider as the chat/ });
    expect(same).toBeChecked();
    fireEvent.click(screen.getByRole("radio", { name: /Mix providers/ }));
    expect(useSettingsStore.getState().settings["workflows.agents"]).toBe("mixed");
    expect(mocks.setSetting).toHaveBeenCalledWith("workflows.agents", "mixed");
  });

  it("shows each provider's readiness and stores provider choices", async () => {
    mount();
    expect(await screen.findByTestId("workflow-provider-grok")).toHaveTextContent("Not available");
    expect(screen.getByTestId("workflow-provider-grok")).toHaveTextContent("No fan-out boundary");
    expect(screen.getByLabelText("Allow Grok in workflows")).toBeDisabled();
    expect(screen.getByTestId("workflow-provider-opencode")).toHaveTextContent("Pick a model");
    fireEvent.change(screen.getByLabelText("OpenCode worker model"), { target: { value: "anthropic/claude-sonnet" } });
    expect(screen.getByTestId("workflow-provider-opencode")).toHaveTextContent("Ready");
    fireEvent.click(screen.getByLabelText("Allow Codex in workflows"));
    const providers = JSON.parse(useSettingsStore.getState().settings["workflows.providers"]);
    expect(providers.codex.enabled).toBe(false);
    expect(providers.opencode.model).toBe("anthropic/claude-sonnet");
  });

  it("stores run size, file access, token budget and the composer shortcut", () => {
    mount();
    expect(screen.getByText("Up to 5 agents, 4 at a time, for at most 30 minutes.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("radio", { name: "Large" }));
    expect(screen.getByText("Up to 10 agents, 8 at a time, for at most 60 minutes.")).toBeInTheDocument();
    fireEvent.change(screen.getByPlaceholderText("No cap"), { target: { value: "50000" } });
    fireEvent.click(screen.getByRole("switch", { name: "Workflow button in the composer" }));
    expect(useSettingsStore.getState().settings).toMatchObject({
      "workflows.size": "large",
      "workflows.token_budget": "50000",
      "workflows.composer_button": "true",
    });
  });
});
