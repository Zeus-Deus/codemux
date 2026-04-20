/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, act, cleanup, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { PresetStoreSnapshot, TerminalPreset } from "@/tauri/types";

const mockGetPresets = vi.fn();
const mockApplyPreset = vi.fn();
const mockSetPresetBarVisible = vi.fn().mockResolvedValue(undefined);
// RunButton reaches for these. Return null/empty so the child renders its
// "Set Run" empty state without hitting any real IPC.
const mockGetProjectScripts = vi.fn().mockResolvedValue(null);
const mockGetWorkspaceConfig = vi.fn().mockResolvedValue(null);
const mockRunProjectDevCommand = vi.fn().mockResolvedValue(undefined);

vi.mock("@/tauri/commands", () => ({
  getPresets: (...args: unknown[]) => mockGetPresets(...args),
  applyPreset: (...args: unknown[]) => mockApplyPreset(...args),
  setPresetBarVisible: (...args: unknown[]) => mockSetPresetBarVisible(...args),
  getProjectScripts: (...args: unknown[]) => mockGetProjectScripts(...args),
  getWorkspaceConfig: (...args: unknown[]) => mockGetWorkspaceConfig(...args),
  runProjectDevCommand: (...args: unknown[]) => mockRunProjectDevCommand(...args),
}));

vi.mock("@/tauri/events", () => ({
  onPresetsChanged: () => Promise.resolve(() => {}),
}));

vi.mock("@/lib/toast", () => ({
  toast: { success: vi.fn(), error: vi.fn(), info: vi.fn(), warning: vi.fn() },
}));

const mockSetShowSettings = vi.fn();

vi.mock("@/stores/ui-store", () => ({
  useUIStore: Object.assign(
    vi.fn((selector: (s: Record<string, unknown>) => unknown) => {
      return selector({ setShowSettings: mockSetShowSettings });
    }),
    {
      getState: () => ({ setShowSettings: mockSetShowSettings }),
    },
  ),
}));

// RunButton pulls its own dependencies (getProjectScripts / getWorkspaceConfig
// / useActiveWorkspace). Mock the parts that would otherwise hit unmocked
// modules so we can mount <PresetBar> without dragging in every store.
vi.mock("@/stores/app-store", () => ({
  useActiveWorkspace: () => ({
    workspace_id: "ws-1",
    project_root: "/home/user/myapp",
  }),
}));

import { TooltipProvider } from "@/components/ui/tooltip";
import { PresetBar } from "./preset-bar";
import { toast as mockToast } from "@/lib/toast";

function flushPromises() {
  return act(() => new Promise((r) => setTimeout(r, 0)));
}

function makePreset(overrides: Partial<TerminalPreset> = {}): TerminalPreset {
  return {
    id: "claude-code",
    name: "Claude Code",
    description: null,
    commands: ["claude"],
    working_directory: null,
    launch_mode: "new_tab",
    icon: null,
    pinned: true,
    is_builtin: true,
    auto_run_on_workspace: false,
    auto_run_on_new_tab: false,
    persona: "agent",
    ...overrides,
  };
}

function makeSnapshot(presets: TerminalPreset[]): PresetStoreSnapshot {
  return {
    presets,
    bar_visible: true,
    default_preset_id: null,
  };
}

function renderPresetBar(workspaceId = "ws-1") {
  return render(
    <TooltipProvider>
      <PresetBar workspaceId={workspaceId} />
    </TooltipProvider>,
  );
}

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("PresetBar — preset failure feedback", () => {
  it("shows an error toast when applyPreset rejects with 'X is not installed'", async () => {
    mockGetPresets.mockResolvedValue(makeSnapshot([makePreset()]));
    // Matches the exact shape the Rust backend returns: a plain string
    // from `Err(format!("{} is not installed", binary))`.
    mockApplyPreset.mockRejectedValue("claude is not installed");

    renderPresetBar();
    await flushPromises();

    const button = screen.getByRole("button", { name: /claude code/i });
    await userEvent.click(button);
    await flushPromises();

    expect(mockApplyPreset).toHaveBeenCalledWith("ws-1", "claude-code", undefined);
    expect(mockToast.error).toHaveBeenCalledTimes(1);
    expect(mockToast.error).toHaveBeenCalledWith(
      "Claude Code: claude is not installed",
    );
  });

  it("handles Error objects (not just strings) gracefully", async () => {
    mockGetPresets.mockResolvedValue(
      makeSnapshot([makePreset({ id: "codex", name: "Codex", commands: ["codex"] })]),
    );
    mockApplyPreset.mockRejectedValue(new Error("codex is not installed"));

    renderPresetBar();
    await flushPromises();

    await userEvent.click(screen.getByRole("button", { name: /codex/i }));
    await flushPromises();

    expect(mockToast.error).toHaveBeenCalledWith("Codex: codex is not installed");
  });

  it("falls back to String(err) for unknown rejection shapes", async () => {
    mockGetPresets.mockResolvedValue(
      makeSnapshot([makePreset({ id: "aider", name: "Aider", commands: ["aider"] })]),
    );
    // Something weird — not a string, not an Error. The catch should still
    // produce a human-readable toast instead of showing `[object Object]`
    // unless `String()` would naturally produce that. The test's purpose
    // here is "it doesn't throw and it does call toast.error once".
    mockApplyPreset.mockRejectedValue({ weird: "shape" });

    renderPresetBar();
    await flushPromises();

    await userEvent.click(screen.getByRole("button", { name: /aider/i }));
    await flushPromises();

    expect(mockToast.error).toHaveBeenCalledTimes(1);
    // We don't assert the exact message here — just that the fallback
    // executed without throwing and the preset name is prefixed. Use
    // `vi.mocked()` to tell TypeScript this is a Mock (the real type
    // signature of `toast.error` is `(msg, opts?) => string | number`).
    const call = vi.mocked(mockToast.error).mock.calls[0]?.[0] as string;
    expect(call).toMatch(/^Aider: /);
  });

  it("does NOT show a toast on successful apply", async () => {
    mockGetPresets.mockResolvedValue(makeSnapshot([makePreset()]));
    mockApplyPreset.mockResolvedValue(undefined);

    renderPresetBar();
    await flushPromises();

    await userEvent.click(screen.getByRole("button", { name: /claude code/i }));
    await flushPromises();

    expect(mockApplyPreset).toHaveBeenCalledTimes(1);
    expect(mockToast.error).not.toHaveBeenCalled();
  });

  it("passes 'split_pane' mode when user shift-clicks the preset", async () => {
    mockGetPresets.mockResolvedValue(makeSnapshot([makePreset()]));
    mockApplyPreset.mockResolvedValue(undefined);

    renderPresetBar();
    await flushPromises();

    // userEvent v14's shift-modifier syntax (`{Shift>}...{/Shift}`) does not
    // reliably set `MouseEvent.shiftKey` on the synthetic click event, so we
    // use `fireEvent.click` with an explicit `shiftKey: true` init to match
    // what the real user interaction produces in the browser.
    const button = screen.getByRole("button", { name: /claude code/i });
    fireEvent.click(button, { shiftKey: true });
    await flushPromises();

    expect(mockApplyPreset).toHaveBeenCalledWith("ws-1", "claude-code", "split_pane");
  });
});
