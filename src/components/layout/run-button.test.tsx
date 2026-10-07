/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, act, cleanup, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const mockGetProjectScripts = vi.fn();
const mockGetWorkspaceConfig = vi.fn();
const mockRunProjectDevCommand = vi.fn().mockResolvedValue(undefined);
const mockSetProjectScripts = vi.fn().mockResolvedValue(undefined);
const mockDetectRunCandidates = vi.fn();

vi.mock("@/tauri/commands", () => ({
  getProjectScripts: (...args: unknown[]) => mockGetProjectScripts(...args),
  getWorkspaceConfig: (...args: unknown[]) => mockGetWorkspaceConfig(...args),
  runProjectDevCommand: (...args: unknown[]) => mockRunProjectDevCommand(...args),
  setProjectScripts: (...args: unknown[]) => mockSetProjectScripts(...args),
  detectRunCandidates: (...args: unknown[]) => mockDetectRunCandidates(...args),
}));

const mockSetShowSettings = vi.fn();
let mockShowSettings = false;

vi.mock("@/stores/ui-store", () => ({
  useUIStore: Object.assign(
    vi.fn((selector: (s: Record<string, unknown>) => unknown) => {
      const state = {
        showSettings: mockShowSettings,
        setShowSettings: mockSetShowSettings,
      };
      return selector(state);
    }),
    {
      getState: () => ({
        setShowSettings: mockSetShowSettings,
      }),
    },
  ),
}));

let mockActiveWorkspace: Record<string, unknown> | null = null;

// RunButton was refactored to use the focused primitive selector
// `useActiveWorkspaceProjectRoot` so it doesn't subscribe to the full
// workspace ref (which churns on every backend tick). The mock has to
// expose that hook now; we still keep `useActiveWorkspace` exported in
// case other test paths reach for it.
vi.mock("@/stores/app-store", () => ({
  useActiveWorkspace: () => mockActiveWorkspace,
  useActiveWorkspaceProjectRoot: () =>
    (mockActiveWorkspace?.project_root as string | undefined) ?? null,
}));

import { TooltipProvider } from "@/components/ui/tooltip";
import { RunButton } from "./run-button";

function flushPromises() {
  return act(() => new Promise((r) => setTimeout(r, 0)));
}

function renderRunButton(workspaceId = "ws-1") {
  return render(
    <TooltipProvider>
      <RunButton workspaceId={workspaceId} />
    </TooltipProvider>,
  );
}

function renderSplitRunButton(workspaceId = "ws-1") {
  return render(
    <TooltipProvider>
      <RunButton workspaceId={workspaceId} variant="split" />
    </TooltipProvider>,
  );
}

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  mockGetProjectScripts.mockResolvedValue(null);
  mockGetWorkspaceConfig.mockResolvedValue(null);
  mockSetProjectScripts.mockResolvedValue(undefined);
  mockDetectRunCandidates.mockResolvedValue([
    { command: "pnpm run dev", source: "package.json" },
    { command: "cargo run", source: "Cargo.toml" },
  ]);
  mockShowSettings = false;
  mockActiveWorkspace = {
    workspace_id: "ws-1",
    project_root: "/home/user/myapp",
  };
});

describe("RunButton", () => {
  it('shows "Set Run" when no run command configured', async () => {
    renderRunButton();
    await flushPromises();
    expect(screen.getByText("Set Run")).toBeInTheDocument();
  });

  it('shows "Run" when run command is configured via DB', async () => {
    mockGetProjectScripts.mockResolvedValue({
      setup: [],
      teardown: [],
      run: "npm run dev",
    });
    renderRunButton();
    await flushPromises();
    expect(screen.getByText("Run")).toBeInTheDocument();
  });

  it('shows "Run" when run command is configured via file config', async () => {
    mockGetWorkspaceConfig.mockResolvedValue({
      setup: [],
      teardown: [],
      run: "yarn dev",
    });
    renderRunButton();
    await flushPromises();
    expect(screen.getByText("Run")).toBeInTheDocument();
  });

  it("file config takes precedence over DB scripts", async () => {
    mockGetWorkspaceConfig.mockResolvedValue({
      setup: [],
      teardown: [],
      run: "file-cmd",
    });
    mockGetProjectScripts.mockResolvedValue({
      setup: [],
      teardown: [],
      run: "db-cmd",
    });
    renderRunButton();
    await flushPromises();
    // Should show "Run" (configured), and the file command in the dropdown
    expect(screen.getByText("Run")).toBeInTheDocument();
  });

  it('click "Set Run" opens settings to projects section', async () => {
    renderRunButton();
    await flushPromises();
    await userEvent.click(screen.getByText("Set Run"));
    expect(mockSetShowSettings).toHaveBeenCalledWith(true, "projects");
  });

  it('click "Run" calls runProjectDevCommand', async () => {
    mockGetProjectScripts.mockResolvedValue({
      setup: [],
      teardown: [],
      run: "npm run dev",
    });
    renderRunButton();
    await flushPromises();
    await userEvent.click(screen.getByText("Run"));
    expect(mockRunProjectDevCommand).toHaveBeenCalledWith("ws-1");
  });

  it("shows shortcut badge", async () => {
    renderRunButton();
    await flushPromises();
    expect(screen.getByText("Ctrl+Shift+G")).toBeInTheDocument();
  });

  it("legacy default renders the standalone gear button that opens configure", async () => {
    const { container } = renderRunButton();
    await flushPromises();
    // Original shape: ghost Run/Set Run button + a second, standalone gear
    // Button (no aria-label — pre-change markup, kept byte-identical).
    const buttons = container.querySelectorAll("button");
    expect(buttons).toHaveLength(2);
    // No split-caret in legacy mode.
    expect(
      screen.queryByRole("button", { name: "Configure run command" }),
    ).toBeNull();
    await userEvent.click(buttons[1]);
    expect(mockSetShowSettings).toHaveBeenCalledWith(true, "projects");
  });

  it('falls back to "Set Run" when no project root', async () => {
    mockActiveWorkspace = {
      workspace_id: "ws-1",
      project_root: null,
    };
    renderRunButton();
    await flushPromises();
    expect(screen.getByText("Set Run")).toBeInTheDocument();
  });

  it("treats empty/whitespace run command as unconfigured", async () => {
    mockGetProjectScripts.mockResolvedValue({
      setup: [],
      teardown: [],
      run: "   ",
    });
    renderRunButton();
    await flushPromises();
    expect(screen.getByText("Set Run")).toBeInTheDocument();
  });
});

// GUI-chrome shape (`variant="split"`, passed only by title-bar.tsx).
// The legacy default above stays byte-identical for the flag-off PresetBar.
describe("RunButton — split variant", () => {
  it('main segment shows "Set Run" and opens the popover with detected commands', async () => {
    renderSplitRunButton();
    await flushPromises();
    await userEvent.click(screen.getByText("Set Run"));
    expect(await screen.findByText("pnpm run dev")).toBeInTheDocument();
    expect(screen.getByText("Cargo.toml")).toBeInTheDocument();
    expect(mockDetectRunCandidates).toHaveBeenCalledWith("/home/user/myapp");
    // Stays in the workspace instead of jumping to full-screen Settings.
    expect(mockSetShowSettings).not.toHaveBeenCalled();
    expect(mockRunProjectDevCommand).not.toHaveBeenCalled();
  });

  it('main segment shows "Run" and runs the dev command when configured', async () => {
    mockGetProjectScripts.mockResolvedValue({
      setup: [],
      teardown: [],
      run: "npm run dev",
    });
    renderSplitRunButton();
    await flushPromises();
    await userEvent.click(screen.getByText("Run"));
    expect(mockRunProjectDevCommand).toHaveBeenCalledWith("ws-1");
  });

  it("caret segment opens the popover even when a run command is set", async () => {
    mockGetProjectScripts.mockResolvedValue({
      setup: [],
      teardown: [],
      run: "npm run dev",
    });
    renderSplitRunButton();
    await flushPromises();
    await userEvent.click(
      screen.getByRole("button", { name: "Configure run command" }),
    );
    // The field starts from the current command so it can be edited.
    expect(await screen.findByLabelText("Run command")).toHaveValue("npm run dev");
    expect(mockSetShowSettings).not.toHaveBeenCalled();
    // Caret never runs the command — only the main segment does.
    expect(mockRunProjectDevCommand).not.toHaveBeenCalled();
  });

  it("picking a detected command saves it, keeps other scripts and runs it", async () => {
    mockGetProjectScripts.mockResolvedValue({
      setup: ["pnpm install"],
      teardown: [],
      run: null,
      worktree_includes: [".env"],
    });
    renderSplitRunButton();
    await flushPromises();
    await userEvent.click(screen.getByText("Set Run"));
    await userEvent.click(await screen.findByRole("button", { name: /pnpm run dev/ }));
    expect(mockSetProjectScripts).toHaveBeenCalledWith("/home/user/myapp", {
      setup: ["pnpm install"],
      teardown: [],
      worktree_includes: [".env"],
      run: "pnpm run dev",
    });
    expect(mockRunProjectDevCommand).toHaveBeenCalledWith("ws-1");
    await waitFor(() => expect(screen.getByText("Run")).toBeInTheDocument());
    expect(screen.queryByLabelText("Run command")).toBeNull();
  });

  it("a typed command is saved and run on Enter", async () => {
    renderSplitRunButton();
    await flushPromises();
    await userEvent.click(screen.getByText("Set Run"));
    await userEvent.type(await screen.findByLabelText("Run command"), "just serve{Enter}");
    expect(mockSetProjectScripts).toHaveBeenCalledWith(
      "/home/user/myapp",
      expect.objectContaining({ run: "just serve" }),
    );
    expect(mockRunProjectDevCommand).toHaveBeenCalledWith("ws-1");
  });

  it("keeps the popover open and shows the error when saving fails", async () => {
    mockSetProjectScripts.mockRejectedValue("database is locked");
    renderSplitRunButton();
    await flushPromises();
    await userEvent.click(screen.getByText("Set Run"));
    await userEvent.click(await screen.findByRole("button", { name: /cargo run/ }));
    expect(await screen.findByText("database is locked")).toBeInTheDocument();
    expect(mockRunProjectDevCommand).not.toHaveBeenCalled();
  });

  it("More settings opens Settings > Projects", async () => {
    renderSplitRunButton();
    await flushPromises();
    await userEvent.click(screen.getByText("Set Run"));
    await userEvent.click(await screen.findByText("More settings…"));
    expect(mockSetShowSettings).toHaveBeenCalledWith(true, "projects");
  });

  it("points at the config file instead of offering commands it would override", async () => {
    mockGetWorkspaceConfig.mockResolvedValue({
      setup: [],
      teardown: [],
      run: null,
      worktree_includes: [],
    });
    mockGetProjectScripts.mockResolvedValue({
      setup: [],
      teardown: [],
      run: "db-cmd",
      worktree_includes: [],
    });
    renderSplitRunButton();
    await flushPromises();
    // The backend ignores DB scripts once a config file exists.
    await userEvent.click(screen.getByText("Set Run"));
    expect(await screen.findByText(".codemux/config.json")).toBeInTheDocument();
    expect(screen.queryByLabelText("Run command")).toBeNull();
    expect(mockDetectRunCandidates).not.toHaveBeenCalled();
  });

  it("re-reads the run command when the active project changes", async () => {
    mockGetProjectScripts.mockImplementation(async (path: string) => ({
      setup: [],
      teardown: [],
      run: path === "/home/user/myapp" ? "npm run dev" : null,
      worktree_includes: [],
    }));
    const { rerender } = renderSplitRunButton();
    await flushPromises();
    expect(screen.getByText("Run")).toBeInTheDocument();
    mockActiveWorkspace = { workspace_id: "ws-2", project_root: "/home/user/other" };
    rerender(
      <TooltipProvider>
        <RunButton workspaceId="ws-2" variant="split" />
      </TooltipProvider>,
    );
    await flushPromises();
    expect(screen.getByText("Set Run")).toBeInTheDocument();
  });

  it("has no inline keyboard-shortcut badge in the DOM", async () => {
    renderSplitRunButton();
    await flushPromises();
    // The badge moved into the main segment's tooltip (mock-faithful
    // restyle) — it must not render as static DOM text any more.
    expect(screen.queryByText("Ctrl+Shift+G")).toBeNull();
  });

  it("shows the shortcut in the main segment's tooltip when unconfigured", async () => {
    renderSplitRunButton();
    await flushPromises();
    await userEvent.hover(screen.getByText("Set Run"));
    // Radix Tooltip renders both a visible content node and a visually
    // hidden (sr-only) duplicate with identical text — `getAllByText`
    // avoids the "multiple elements" ambiguity `getByText` would throw.
    await waitFor(() =>
      expect(
        screen.getAllByText("Set Run · Ctrl+Shift+G").length,
      ).toBeGreaterThan(0),
    );
  });

  it("shows the configured command + shortcut in the main segment's tooltip when configured", async () => {
    mockGetProjectScripts.mockResolvedValue({
      setup: [],
      teardown: [],
      run: "npm run dev",
    });
    renderSplitRunButton();
    await flushPromises();
    await userEvent.hover(screen.getByText("Run"));
    await waitFor(() =>
      expect(
        screen.getAllByText("npm run dev · Ctrl+Shift+G").length,
      ).toBeGreaterThan(0),
    );
  });

  it("renders a borderless 28px run segment plus a 20px caret segment", async () => {
    const { container } = renderSplitRunButton();
    await flushPromises();
    const outer = container.firstElementChild;
    // No chip: in the titlebar band this control carries no border and no
    // resting fill, only a hover fill, like the panel toggle opposite it.
    expect(outer?.className).not.toContain("border");
    expect(outer?.className).not.toContain("bg-secondary");
    expect(outer?.className).toContain("h-7");
    const buttons = container.querySelectorAll("button");
    expect(buttons).toHaveLength(2);
    // Caret segment is 20px wide (w-5) and carries the aria-label.
    const caret = screen.getByRole("button", { name: "Configure run command" });
    expect(caret.className).toContain("w-5");
  });
});
