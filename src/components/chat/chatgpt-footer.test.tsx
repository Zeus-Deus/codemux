import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ComposerFooter } from "./ComposerFooter";
import { useChatGptStore } from "@/stores/chatgpt-store";
import { openUrl } from "@tauri-apps/plugin-opener";
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn().mockResolvedValue(undefined) }));
vi.mock("./pickers/ModelPicker", () => ({ ModelPicker: () => null }));
vi.mock("./pickers/MultiProviderModelPicker", () => ({ MultiProviderModelPicker: () => null }));
vi.mock("./pickers/ReasoningPicker", () => ({ ReasoningPicker: () => null }));
vi.mock("./pickers/PermissionModePicker", () => ({ PermissionModePicker: () => null }));
afterEach(() => { cleanup(); useChatGptStore.setState({ status: null }); vi.clearAllMocks(); });
it("identifies ChatGPT plan use beside Codex controls and links to real usage settings", () => {
  useChatGptStore.setState({ status: { phase: "connected", attemptId: null, email: null,
    error: null, profiles: [], activeProfileId: "test-profile", welcomePending: false, installed: true } });
  render(<TooltipProvider><ComposerFooter provider="codex" model={null} permissionMode={null}
    effort={null} contextWindow={null} activeModel={null} effortLabelMap={{}} permissionModes={null}
    ultrathinkInBodyText={false} streaming={false} canSubmit={false} showProviderPicker={false} mode="default"
    onProviderModelChange={vi.fn()} onModelChange={vi.fn()} onPermissionModeChange={vi.fn()}
    onEffortChange={vi.fn()} onContextWindowChange={vi.fn()} onSubmit={vi.fn()} onStop={vi.fn()} controlsDisabled={false} /></TooltipProvider>);
  expect(screen.getByText("Using ChatGPT plan")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Manage usage" }));
  expect(openUrl).toHaveBeenCalledWith("https://chatgpt.com/settings/usage");
});
